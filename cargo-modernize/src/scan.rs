//! Static analysis: parse every source file with `syn`, resolve paths through
//! `use` imports, and match them against the active rules. Structural detectors
//! and their rewrites live in `rewrite.rs`.

use crate::config::Config;
use crate::ecosystem::{self, Ecosystem};
use crate::project::{LockPkg, Package};
use crate::rules::{self, FEATURES, FixSafety, Kind, Level, Rule};
use anyhow::Result;
use proc_macro2::{LineColumn, Span};
use semver::Version;
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use syn::ext::IdentExt;
use syn::punctuated::Punctuated;
use syn::spanned::Spanned;
use syn::visit::{self, Visit};
use syn::{Expr, Lit};

/// Built-in structural detectors that `kind = "pattern"` rules can name.
pub const PATTERNS: &[&str] = &[
    "let-else",
    "let-chains",
    "inline-format-args",
    "is-some-and",
    "is-none-or",
    "io-error-other",
    "mem-take",
    "matches-macro",
    "split-once",
    "as-deref",
    "derive-default-enum",
    "non-exhaustive",
    "trait-upcasting",
    "bool-ok-or",
    "bool-toggle",
    "trim-prefix",
    "lossy-owned",
    "vec-from-fn",
    "local-key-update",
    "max-value-fn",
    "try-macro",
    "lazy-static",
    "crossbeam-scope",
    "addr-of",
];

pub struct PackageReport {
    pub name: String,
    pub version: String,
    pub edition: u16,
    pub rust_version: String,
    pub findings: Vec<Finding>,
}

pub struct Finding {
    pub rule: Rule,
    pub level: Level,
    /// Relative to the workspace root, for display.
    pub file: PathBuf,
    /// 1-based start.
    pub line: usize,
    pub column: usize,
    /// 1-based end (exclusive column), for carets.
    pub end_line: usize,
    pub end_column: usize,
    /// The full first line of the span.
    pub source_line: String,
    pub snippet: String,
    /// Finding-specific detail, e.g. which feature gate.
    pub detail: Option<String>,
    pub note: Option<String>,
    pub fix: Option<Fix>,
}

#[derive(Clone)]
pub struct Fix {
    /// Needs `--dangerous`: may change behavior or break the build.
    pub dangerous: bool,
    pub edits: Vec<Edit>,
}

/// Replace `old` at bytes `start..end` of `path` with `new`.
#[derive(Clone, Debug)]
pub struct Edit {
    pub path: PathBuf,
    pub start: usize,
    pub end: usize,
    pub old: String,
    pub new: String,
}

pub(crate) struct Active<'r> {
    pub rule: &'r Rule,
    pub note: Option<String>,
    pub beta: bool,
    pub upcoming: bool,
}

/// A raw match before levels are resolved.
pub(crate) struct Hit {
    pub rule: usize,
    pub file: PathBuf,
    pub start: LineColumn,
    pub end: LineColumn,
    pub source_line: String,
    pub detail: Option<String>,
    pub note: Option<String>,
    pub beta: bool,
    pub upcoming: bool,
    pub edits: Option<Vec<Edit>>,
}

pub struct Context<'a> {
    pub ws_root: &'a Path,
    pub rules: &'a [Rule],
    pub config: &'a Config,
    pub rustc: Option<&'a Version>,
    pub ecosystem: &'a Ecosystem,
    pub lock: &'a [LockPkg],
}

pub fn analyze(pkg: &Package, cx: &Context) -> Result<PackageReport> {
    let active: Vec<Active> = cx
        .rules
        .iter()
        .filter(|r| {
            r.scope
                .as_ref()
                .is_none_or(|s| pkg.root.starts_with(s) || s.starts_with(&pkg.root))
        })
        .filter_map(|rule| {
            rule.applies(pkg, cx.rustc).map(|a| Active {
                rule,
                note: a.note,
                beta: a.beta,
                upcoming: a.upcoming,
            })
        })
        .collect();

    // Code refers to dependencies by their (possibly renamed) key; rules use the real crate name.
    let root_map: HashMap<String, String> = pkg
        .deps
        .iter()
        .filter(|d| d.rename.is_some())
        .map(|d| (d.key().replace('-', "_"), d.name.replace('-', "_")))
        .collect();

    let mut hits = manifest_hits(pkg, &active);
    hits.extend(ecosystem_hits(pkg, cx, &active));
    for file in source_files(&pkg.root) {
        let Ok(text) = std::fs::read_to_string(&file) else {
            continue;
        };
        let ast = match syn::parse_file(&text) {
            Ok(ast) => ast,
            Err(e) => {
                eprintln!("warning: skipping {}: {e}", file.display());
                continue;
            }
        };
        let mut uses = UseCollector::default();
        uses.visit_file(&ast);
        let mut s = Scanner::new(&active, &root_map, &file, &text, uses.leaves, pkg, cx.rustc);
        s.collect_enums(&ast);
        s.visit_file(&ast);
        hits.extend(s.out);
    }

    let findings = hits
        .into_iter()
        .filter_map(|h| {
            let a = &active[h.rule];
            if a.rule
                .scope
                .as_ref()
                .is_some_and(|s| !h.file.starts_with(s))
            {
                return None;
            }
            let mut level = cx.config.level(a.rule, &h.file);
            if (h.beta || h.upcoming) && level > Level::Suggest {
                level = Level::Suggest; // can't act on it with a stable toolchain yet
            }
            // A fix that needs a newer Rust than the crate declares raises its MSRV: dangerous.
            let fix = h
                .edits
                .filter(|e| !e.is_empty() && !h.beta && !h.upcoming)
                .map(|edits| Fix {
                    dangerous: a.rule.fix == FixSafety::Dangerous
                        || h.note
                            .as_deref()
                            .is_some_and(|n| n.contains("rust-version")),
                    edits,
                });
            (level != Level::Off).then(|| Finding {
                rule: a.rule.clone(),
                level,
                file: h
                    .file
                    .strip_prefix(cx.ws_root)
                    .unwrap_or(&h.file)
                    .to_path_buf(),
                line: h.start.line,
                column: h.start.column + 1,
                end_line: h.end.line,
                end_column: h.end.column + 1,
                snippet: h.source_line.trim().to_owned(),
                source_line: h.source_line,
                detail: h.detail,
                note: h.note,
                fix,
            })
        })
        .collect();

    Ok(PackageReport {
        name: pkg.name.clone(),
        version: pkg.version.clone(),
        edition: pkg.edition,
        rust_version: pkg.rust_version_str(),
        findings,
    })
}

/// Scan a single source string and return findings.
///
/// This is a simplified version of [`analyze`] for LSP/incremental use:
/// it parses `source` directly instead of reading from disk, and skips
/// manifest/ecosystem hits.
pub fn scan_source(
    path: &Path,
    source: &str,
    rules: &[Rule],
    config: &Config,
    rustc: Option<&Version>,
) -> Vec<Finding> {
    // Minimal package for rule applicability checks.
    let pkg = Package {
        name: "unknown".into(),
        version: "0.0.0".into(),
        root: path.parent().unwrap_or(Path::new("")).to_path_buf(),
        manifest_path: PathBuf::new(),
        workspace_manifest: PathBuf::new(),
        edition: 2024,
        rust_version: None,
        deps: Vec::new(),
    };

    let active: Vec<Active> = rules
        .iter()
        .filter_map(|rule| {
            rule.applies(&pkg, rustc).map(|a| Active {
                rule,
                note: a.note,
                beta: a.beta,
                upcoming: a.upcoming,
            })
        })
        .collect();

    let root_map: HashMap<String, String> = HashMap::new();

    let Ok(ast) = syn::parse_file(source) else {
        return Vec::new();
    };

    let mut uses = UseCollector::default();
    uses.visit_file(&ast);

    let mut s = Scanner::new(&active, &root_map, path, source, uses.leaves, &pkg, rustc);
    s.collect_enums(&ast);
    s.visit_file(&ast);
    let hits = s.out;

    hits.into_iter()
        .filter_map(|h| {
            let a = &active[h.rule];
            if a.rule.scope.as_ref().is_some_and(|s| !path.starts_with(s)) {
                return None;
            }
            let mut level = config.level(a.rule, path);
            if (h.beta || h.upcoming) && level > Level::Suggest {
                level = Level::Suggest;
            }
            let fix = h
                .edits
                .filter(|e| !e.is_empty() && !h.beta && !h.upcoming)
                .map(|edits| Fix {
                    dangerous: a.rule.fix == FixSafety::Dangerous
                        || h.note
                            .as_deref()
                            .is_some_and(|n| n.contains("rust-version")),
                    edits,
                });
            (level != Level::Off).then(|| Finding {
                rule: a.rule.clone(),
                level,
                file: path.to_path_buf(),
                line: h.start.line,
                column: h.start.column + 1,
                end_line: h.end.line,
                end_column: h.end.column + 1,
                snippet: h.source_line.trim().to_owned(),
                source_line: h.source_line,
                detail: h.detail,
                note: h.note,
                fix,
            })
        })
        .collect()
}

fn source_files(root: &Path) -> Vec<PathBuf> {
    walkdir::WalkDir::new(root)
        .into_iter()
        .filter_entry(|e| {
            let name = e.file_name().to_string_lossy();
            if e.depth() == 0 {
                return true;
            }
            // Skip build output, VCS dirs, and nested packages (they're analyzed on their own).
            !(e.file_type().is_dir()
                && (name == "target"
                    || name.starts_with('.')
                    || e.path().join("Cargo.toml").exists()))
        })
        .filter_map(|e| e.ok())
        .filter(|e| e.file_type().is_file() && e.path().extension().is_some_and(|x| x == "rs"))
        .map(|e| e.into_path())
        .collect()
}

/// Byte offset of a (1-based line, 0-based char column) position.
pub(crate) fn byte_offset(text: &str, line_starts: &[usize], lc: LineColumn) -> usize {
    let Some(&ls) = line_starts.get(lc.line.saturating_sub(1)) else {
        return text.len();
    };
    text[ls..]
        .char_indices()
        .nth(lc.column)
        .map_or(text.len(), |(b, _)| ls + b)
}

pub(crate) fn line_starts(text: &str) -> Vec<usize> {
    std::iter::once(0)
        .chain(text.match_indices('\n').map(|(i, _)| i + 1))
        .collect()
}

fn lc(line: usize, column: usize) -> LineColumn {
    LineColumn { line, column }
}

// ---------------------------------------------------------------- Cargo.toml

fn manifest_line_hit(
    a: &Active,
    rule: usize,
    file: &Path,
    lines: &[String],
    idx: usize,
    edits: Option<Vec<Edit>>,
    detail: Option<String>,
) -> Hit {
    let line = lines.get(idx).cloned().unwrap_or_default();
    let indent = line.len() - line.trim_start().len();
    Hit {
        rule,
        file: file.to_path_buf(),
        start: lc(idx + 1, indent),
        end: lc(idx + 1, line.trim_end().chars().count()),
        source_line: line,
        detail,
        note: a.note.clone(),
        beta: a.beta,
        upcoming: a.upcoming,
        edits,
    }
}

fn manifest_hits(pkg: &Package, active: &[Active]) -> Vec<Hit> {
    let mut out = Vec::new();
    let text = std::fs::read_to_string(&pkg.manifest_path).unwrap_or_default();
    for (i, a) in active.iter().enumerate() {
        match a.rule.kind {
            Kind::Dependency => {
                let dep = pkg.dep(a.rule.krate.as_deref().unwrap()).unwrap();
                let Some((mut path, mut lines, mut idx)) =
                    locate_dep(&pkg.manifest_path, &text, dep.key())
                else {
                    continue;
                };
                let mut file_text = text.clone();
                // `dep = { workspace = true }`: the version lives in the workspace manifest.
                if lines[idx].contains("workspace") && pkg.workspace_manifest != pkg.manifest_path {
                    let root_text =
                        std::fs::read_to_string(&pkg.workspace_manifest).unwrap_or_default();
                    if let Some(found) = locate_dep(&pkg.workspace_manifest, &root_text, dep.key())
                    {
                        (path, lines, idx) = found;
                        file_text = root_text;
                    }
                }
                let edits = a.rule.to_version.as_ref().and_then(|new| {
                    let (line, col, old) = version_string(&lines, idx)?;
                    let starts = line_starts(&file_text);
                    let start = byte_offset(&file_text, &starts, lc(line + 1, col));
                    (old != *new).then(|| {
                        vec![Edit {
                            path: path.clone(),
                            start,
                            end: start + old.len(),
                            old,
                            new: new.clone(),
                        }]
                    })
                });
                out.push(manifest_line_hit(a, i, &path, &lines, idx, edits, None));
            }
            Kind::Edition => {
                let lines: Vec<String> = text.lines().map(str::to_owned).collect();
                let idx = lines
                    .iter()
                    .position(|l| l.trim_start().starts_with("edition"))
                    .unwrap_or(0);
                out.push(manifest_line_hit(
                    a,
                    i,
                    &pkg.manifest_path,
                    &lines,
                    idx,
                    None,
                    None,
                ));
            }
            _ => {}
        }
    }
    out
}

/// Dependency findings from the ecosystem data: status of direct dependencies,
/// newer major versions, and hard problems in transitive dependencies.
fn ecosystem_hits(pkg: &Package, cx: &Context, active: &[Active]) -> Vec<Hit> {
    let rule_for = |status: &str| {
        active.iter().position(|a| {
            a.rule.kind == Kind::Ecosystem && a.rule.pattern.as_deref() == Some(status)
        })
    };
    // Hand-written dependency rules carry better advice; don't double-report those crates.
    let covered: HashSet<&str> = active
        .iter()
        .filter(|a| a.rule.kind == Kind::Dependency)
        .filter_map(|a| a.rule.krate.as_deref())
        .collect();
    let text = std::fs::read_to_string(&pkg.manifest_path).unwrap_or_default();
    let mut out = Vec::new();
    let mut seen = HashSet::new();

    for dep in &pkg.deps {
        if covered.contains(dep.name.as_str()) || !seen.insert(dep.name.as_str()) {
            continue;
        }
        let Some(entry) = cx.ecosystem.crates.get(&dep.name) else {
            continue;
        };
        let Some((path, lines, idx)) = locate_dep(&pkg.manifest_path, &text, dep.key()) else {
            continue;
        };
        if let Some(status) = entry.status.as_deref() {
            if let Some(r) = rule_for(status) {
                out.push(manifest_line_hit(
                    &active[r],
                    r,
                    &path,
                    &lines,
                    idx,
                    None,
                    Some(entry.advice()),
                ));
            }
            continue;
        }
        // Newer semver-incompatible release?
        let (Some(current), Some(latest)) = (
            dep.version(),
            entry.latest.as_deref().and_then(|l| Version::parse(l).ok()),
        ) else {
            continue;
        };
        let compatible =
            semver::VersionReq::parse(&format!("^{current}")).is_ok_and(|req| req.matches(&latest));
        if latest > current && latest.pre.is_empty() && !compatible {
            if let Some(r) = rule_for("outdated") {
                let detail = format!("On {current}; the latest release is {latest}.");
                out.push(manifest_line_hit(
                    &active[r],
                    r,
                    &path,
                    &lines,
                    idx,
                    None,
                    Some(detail),
                ));
            }
        }
    }

    // Transitive: walk Cargo.lock from this package.
    let (Some(r), Some(root)) = (
        rule_for("transitive"),
        cx.lock
            .iter()
            .find(|l| l.name == pkg.name && l.version == pkg.version),
    ) else {
        return out;
    };
    let lock_path = cx.ws_root.join("Cargo.lock");
    let lock_text = std::fs::read_to_string(&lock_path).unwrap_or_default();
    let lock_lines: Vec<String> = lock_text.lines().map(str::to_owned).collect();
    let find = |spec: &str| -> Option<&LockPkg> {
        let mut parts = spec.split(' ');
        let (name, version) = (parts.next()?, parts.next());
        cx.lock
            .iter()
            .find(|l| l.name == name && version.is_none_or(|v| l.version == v))
    };
    let direct: HashSet<&str> = pkg.deps.iter().map(|d| d.name.as_str()).collect();
    let mut parent: HashMap<(&str, &str), (&str, &str)> = HashMap::new();
    let mut queue = std::collections::VecDeque::from([root]);
    let mut visited = HashSet::from([(root.name.as_str(), root.version.as_str())]);
    while let Some(p) = queue.pop_front() {
        for spec in &p.dependencies {
            let Some(d) = find(spec) else { continue };
            if visited.insert((d.name.as_str(), d.version.as_str())) {
                parent.insert((&d.name, &d.version), (&p.name, &p.version));
                queue.push_back(d);
            }
        }
    }
    let mut reported = HashSet::new();
    for &(name, version) in &visited {
        if direct.contains(name)
            || name == pkg.name
            || covered.contains(name)
            || !reported.insert(name)
        {
            continue;
        }
        let Some(entry) = cx.ecosystem.crates.get(name) else {
            continue;
        };
        let Some(status) = entry
            .status
            .as_deref()
            .filter(|s| ecosystem::HARD.contains(s))
        else {
            continue;
        };
        let mut chain = vec![format!("{name} {version}")];
        let mut cur = (name, version);
        while let Some(&p) = parent.get(&cur) {
            if p.0 == pkg.name {
                break;
            }
            chain.push(format!("{} {}", p.0, p.1));
            cur = p;
        }
        chain.reverse();
        let line = lock_lines
            .iter()
            .position(|l| *l == format!("name = \"{name}\""))
            .unwrap_or(0);
        let detail = format!(
            "`{name}` is {status}; pulled in via {}. {}",
            chain.join(" → "),
            entry.advice()
        );
        out.push(manifest_line_hit(
            &active[r],
            r,
            &lock_path,
            &lock_lines,
            line,
            None,
            Some(detail),
        ));
    }
    out.sort_by_key(|h| (h.file.clone(), h.start.line));
    out
}

fn locate_dep(path: &Path, text: &str, key: &str) -> Option<(PathBuf, Vec<String>, usize)> {
    let lines: Vec<String> = text.lines().map(str::to_owned).collect();
    let mut in_deps = false;
    for (i, raw) in lines.iter().enumerate() {
        let l = raw.trim();
        if l.starts_with('[') {
            let header = l.trim_matches(['[', ']']).trim();
            if header.ends_with(&format!("dependencies.{key}")) {
                return Some((path.to_path_buf(), lines, i));
            }
            in_deps = header.ends_with("dependencies");
            continue;
        }
        if in_deps {
            let k = l
                .split(['=', '.'])
                .next()
                .unwrap_or("")
                .trim()
                .trim_matches('"');
            if k == key {
                return Some((path.to_path_buf(), lines, i));
            }
        }
    }
    None
}

/// Finds the quoted version requirement for the dependency at `idx`:
/// `dep = "1"`, `dep = { version = "1" }`, or a `version = "1"` line under `[dependencies.dep]`.
fn version_string(lines: &[String], idx: usize) -> Option<(usize, usize, String)> {
    let quoted_after = |line: &str, from: usize| -> Option<(usize, String)> {
        let start = from + line[from..].find('"')? + 1;
        let end = start + line[start..].find('"')?;
        Some((line[..start].chars().count(), line[start..end].to_owned()))
    };
    let line = &lines[idx];
    if line.trim_start().starts_with('[') {
        for (i, l) in lines.iter().enumerate().skip(idx + 1) {
            if l.trim_start().starts_with('[') {
                break;
            }
            if l.trim_start().starts_with("version") {
                return quoted_after(l, l.find('=')?).map(|(c, s)| (i, c, s));
            }
        }
        return None;
    }
    let from = match line.find("version") {
        Some(v) => v + line[v..].find('=')?,
        None if line[line.find('=')?..]
            .trim_start_matches('=')
            .trim_start()
            .starts_with('"') =>
        {
            line.find('=')?
        }
        None => return None,
    };
    quoted_after(line, from).map(|(c, s)| (idx, c, s))
}

// ---------------------------------------------------------------- source

pub(crate) type Seg = (String, Span);

pub(crate) struct UseLeaf {
    segs: Vec<Seg>,
    alias: Option<String>,
    glob: bool,
    leading_colon: bool,
    /// The whole `use …;` item, when this leaf is its only import.
    sole_item: Option<(LineColumn, LineColumn)>,
}

#[derive(Default)]
struct UseCollector {
    leaves: Vec<UseLeaf>,
}

impl<'ast> Visit<'ast> for UseCollector {
    fn visit_item_use(&mut self, u: &'ast syn::ItemUse) {
        let before = self.leaves.len();
        flatten(
            &u.tree,
            Vec::new(),
            u.leading_colon.is_some(),
            &mut self.leaves,
        );
        if self.leaves.len() == before + 1 {
            self.leaves[before].sole_item = Some((u.span().start(), u.span().end()));
        }
    }
}

fn flatten(tree: &syn::UseTree, mut prefix: Vec<Seg>, lc: bool, out: &mut Vec<UseLeaf>) {
    use syn::UseTree::*;
    let leaf = |segs, alias, glob| UseLeaf {
        segs,
        alias,
        glob,
        leading_colon: lc,
        sole_item: None,
    };
    match tree {
        Path(p) => {
            prefix.push((p.ident.unraw().to_string(), p.ident.span()));
            flatten(&p.tree, prefix, lc, out);
        }
        Name(n) if n.ident == "self" => {
            let alias = prefix.last().map(|s| s.0.clone());
            out.push(leaf(prefix, alias, false));
        }
        Name(n) => {
            prefix.push((n.ident.unraw().to_string(), n.ident.span()));
            out.push(leaf(prefix, Some(n.ident.unraw().to_string()), false));
        }
        Rename(r) => {
            prefix.push((r.ident.unraw().to_string(), r.ident.span()));
            let alias = Some(r.rename.unraw().to_string()).filter(|a| a != "_");
            out.push(leaf(prefix, alias, false));
        }
        Glob(_) => out.push(leaf(prefix, None, true)),
        Group(g) => g
            .items
            .iter()
            .for_each(|t| flatten(t, prefix.clone(), lc, out)),
    }
}

pub(crate) struct EnumInfo {
    /// Where to insert `, Default` in an existing `#[derive(..)]`.
    pub derive_close: Option<LineColumn>,
    /// Start of the item (its first attribute or keyword), for inserting attributes.
    pub start: LineColumn,
    /// Variant name → where to insert `#[default] `.
    pub variants: HashMap<String, LineColumn>,
}

pub(crate) struct Scanner<'a> {
    pub rules: &'a [Active<'a>],
    root_map: &'a HashMap<String, String>,
    aliases: HashMap<String, Vec<String>>,
    /// Fully-resolved import paths (glob imports contribute their prefix).
    imports: Vec<Vec<String>>,
    uses: Vec<UseLeaf>,
    pub path: &'a Path,
    pub text: &'a str,
    pub line_starts: Vec<usize>,
    pkg: &'a Package,
    rustc: Option<&'a Version>,
    pub out: Vec<Hit>,
    seen: HashSet<(String, usize, usize)>,
    /// Fixes computed at a call site for the path inside it (keyed by rule and path position).
    pending: HashMap<(usize, usize, usize), Vec<Edit>>,
    /// Macro invocations used as method receivers (rewrites there need parentheses).
    pub receivers: HashSet<(usize, usize)>,
    /// Calls already rewritten by an enclosing expression.
    pub handled: HashSet<(usize, usize)>,
    pub enums: HashMap<String, EnumInfo>,
    /// Where to add a `use`: after the last top-level `use`, or before the first item.
    pub import_at: Option<(LineColumn, bool)>,
}

impl<'a> Scanner<'a> {
    fn new(
        rules: &'a [Active<'a>],
        root_map: &'a HashMap<String, String>,
        path: &'a Path,
        text: &'a str,
        uses: Vec<UseLeaf>,
        pkg: &'a Package,
        rustc: Option<&'a Version>,
    ) -> Self {
        let mut s = Scanner {
            rules,
            root_map,
            aliases: HashMap::new(),
            imports: Vec::new(),
            uses: Vec::new(),
            path,
            text,
            line_starts: line_starts(text),
            pkg,
            rustc,
            out: Vec::new(),
            seen: HashSet::new(),
            pending: HashMap::new(),
            receivers: HashSet::new(),
            handled: HashSet::new(),
            enums: HashMap::new(),
            import_at: None,
        };
        for leaf in &uses {
            let names: Vec<String> = leaf.segs.iter().map(|s| s.0.clone()).collect();
            if names
                .first()
                .is_some_and(|f| matches!(f.as_str(), "crate" | "self" | "super"))
            {
                continue;
            }
            let resolved = s.map_root(names);
            if let (Some(alias), false) = (&leaf.alias, leaf.glob) {
                s.aliases.insert(alias.clone(), resolved.clone());
            }
            s.imports.push(resolved);
        }
        s.uses = uses;
        s
    }

    // ------------------------------------------------------------ source text

    pub fn off(&self, lc: LineColumn) -> usize {
        byte_offset(self.text, &self.line_starts, lc)
    }

    pub fn src(&self, sp: Span) -> &'a str {
        let (a, b) = (self.off(sp.start()), self.off(sp.end()));
        &self.text[a.min(b)..b]
    }

    pub fn src_of(&self, node: &impl Spanned) -> &'a str {
        self.src(node.span())
    }

    pub fn edit(&self, a: LineColumn, b: LineColumn, new: impl Into<String>) -> Edit {
        let (start, end) = (self.off(a), self.off(b));
        Edit {
            path: self.path.to_path_buf(),
            start,
            end,
            old: self.text[start..end].to_owned(),
            new: new.into(),
        }
    }

    pub fn replace(&self, node: &impl Spanned, new: impl Into<String>) -> Edit {
        let sp = node.span();
        self.edit(sp.start(), sp.end(), new)
    }

    /// Deletes `a..b`, and the whole line(s) when nothing else is on them.
    pub fn delete_lines(&self, a: LineColumn, b: LineColumn) -> Edit {
        let (mut start, mut end) = (self.off(a), self.off(b));
        let line_start = self.text[..start].rfind('\n').map_or(0, |i| i + 1);
        let rest_end = self.text[end..]
            .find('\n')
            .map_or(self.text.len(), |i| end + i + 1);
        if self.text[line_start..start].trim().is_empty()
            && self.text[end..rest_end].trim().is_empty()
        {
            start = line_start;
            end = rest_end;
            // Don't leave two blank lines where the item was.
            if self.text[..start].ends_with("\n\n") && self.text[end..].starts_with('\n') {
                end += 1;
            }
        }
        Edit {
            path: self.path.to_path_buf(),
            start,
            end,
            old: self.text[start..end].to_owned(),
            new: String::new(),
        }
    }

    /// Indentation of the line containing `lc`.
    pub fn indent_at(&self, lc: LineColumn) -> String {
        let line = self.text[self.line_starts[lc.line - 1]..]
            .lines()
            .next()
            .unwrap_or("");
        line[..line.len() - line.trim_start().len()].to_owned()
    }

    // ------------------------------------------------------------ resolution

    fn map_root(&self, mut path: Vec<String>) -> Vec<String> {
        if let Some(real) = path.first().and_then(|f| self.root_map.get(f)) {
            path[0] = real.clone();
        }
        path
    }

    /// Resolves a code path through the file's imports. Returns the full path and
    /// the offset such that `resolved[i]` corresponds to `segs[i - offset]`.
    fn resolve(&self, segs: &[Seg], leading_colon: bool) -> Option<(Vec<String>, isize)> {
        let first = &segs.first()?.0;
        if matches!(first.as_str(), "crate" | "self" | "super" | "Self") {
            return None;
        }
        let rest = segs[1..].iter().map(|s| s.0.clone());
        match self.aliases.get(first).filter(|_| !leading_colon) {
            Some(full) => Some((
                full.iter().cloned().chain(rest).collect(),
                full.len() as isize - 1,
            )),
            None => Some((self.map_root(segs.iter().map(|s| s.0.clone()).collect()), 0)),
        }
    }

    /// The name to write for `full` (e.g. `std::sync::LazyLock`), plus the edit that
    /// imports it if needed. Falls back to the full path if the short name is taken.
    pub fn import(&self, full: &str) -> (String, Option<Edit>) {
        let short = full.rsplit("::").next().unwrap_or(full);
        let want: Vec<String> = full.split("::").map(str::to_owned).collect();
        match self.aliases.get(short) {
            Some(existing) if *existing == want => (short.to_owned(), None),
            Some(_) => (full.to_owned(), None),
            None => match self.import_at {
                Some((at, true)) => (
                    short.to_owned(),
                    Some(self.edit(at, at, format!("\nuse {full};"))),
                ),
                Some((at, false)) => (
                    short.to_owned(),
                    Some(self.edit(at, at, format!("use {full};\n\n"))),
                ),
                None => (full.to_owned(), None),
            },
        }
    }

    pub fn resolve_expr_path(&self, e: &Expr) -> Option<Vec<String>> {
        let Expr::Path(p) = e else { return None };
        self.resolve(&path_segs(&p.path), p.path.leading_colon.is_some())
            .map(|(r, _)| r)
    }

    fn import_ok(&self, rule: &Rule) -> bool {
        rule.import_segs.is_empty()
            || rule
                .import_segs
                .iter()
                .any(|req| self.imports.iter().any(|imp| imp.starts_with(req)))
    }

    // ------------------------------------------------------------ hits

    pub fn hit(
        &mut self,
        idx: usize,
        start: LineColumn,
        end: LineColumn,
        edits: Option<Vec<Edit>>,
        detail: Option<String>,
        note: Option<(Option<String>, bool)>,
    ) {
        let a = &self.rules[idx];
        if !self.seen.insert((
            a.rule.id.clone() + detail.as_deref().unwrap_or(""),
            start.line,
            start.column,
        )) {
            return;
        }
        let (note, beta) = note.unwrap_or((a.note.clone(), a.beta));
        let source_line = self
            .text
            .lines()
            .nth(start.line.wrapping_sub(1))
            .unwrap_or("")
            .to_owned();
        self.out.push(Hit {
            rule: idx,
            file: self.path.to_path_buf(),
            start,
            end,
            source_line,
            detail,
            note,
            beta,
            upcoming: beta,
            edits,
        });
    }

    pub fn indices(&self, pred: impl Fn(&Rule) -> bool) -> Vec<usize> {
        self.rules
            .iter()
            .enumerate()
            .filter(|(_, a)| pred(a.rule))
            .map(|(i, _)| i)
            .collect()
    }

    /// Is a pattern rule active (worth computing a rewrite for)?
    pub fn wants(&self, name: &str) -> bool {
        self.rules.iter().any(|a| {
            a.rule.kind == Kind::Pattern
                && a.rule.pattern.as_deref() == Some(name)
                && self.import_ok(a.rule)
        })
    }

    /// Reports a built-in structural pattern over `node`'s span, with an optional rewrite.
    pub fn pattern(&mut self, name: &str, at: Span, edits: Option<Vec<Edit>>) {
        for i in self.indices(|r| r.kind == Kind::Pattern && r.pattern.as_deref() == Some(name)) {
            if self.import_ok(self.rules[i].rule) {
                self.hit(i, at.start(), at.end(), edits.clone(), None, None);
            }
        }
    }

    /// Matches a path against pattern-based rules of the given kinds.
    fn check_path(
        &mut self,
        segs: &[Seg],
        leading_colon: bool,
        kinds: &[Kind],
        sole_use: Option<(LineColumn, LineColumn)>,
        in_use: bool,
    ) {
        let Some((resolved, offset)) = self.resolve(segs, leading_colon) else {
            return;
        };
        for i in self.indices(|r| kinds.contains(&r.kind)) {
            let rule = self.rules[i].rule;
            let Some(pat) = rule.segs.iter().find(|p| resolved.starts_with(p)) else {
                continue;
            };
            // The code segment the rule's last component lines up with. If it's hidden
            // inside an import alias, the `use` itself is reported (and fixed) instead.
            let Some(target) = usize::try_from(pat.len() as isize - 1 - offset).ok() else {
                continue;
            };
            let Some(seg) = segs.get(target) else {
                continue;
            };
            let last = pat.last().unwrap();
            let spelled_fully = offset == 0 && segs.len() >= pat.len();
            let mut edits = self
                .pending
                .remove(&(i, seg.1.start().line, seg.1.start().column));
            if edits.is_none() {
                edits = if let (true, Some((a, b))) = (rule.remove_use, sole_use) {
                    Some(vec![self.delete_lines(a, b)])
                } else if let Some(new) = &rule.replace_with {
                    if spelled_fully {
                        Some(vec![self.edit(
                            segs[0].1.start(),
                            segs[pat.len() - 1].1.end(),
                            new.clone(),
                        )])
                    } else {
                        let tail = new.rsplit("::").next().unwrap_or(new);
                        (seg.0 == *last).then(|| vec![self.replace(&seg.1, tail)])
                    }
                } else if rule.drop_prefix
                    && !in_use
                    && spelled_fully
                    && matches!(segs[0].0.as_str(), "std" | "core")
                    && segs.len() >= 2
                {
                    let rest: Vec<&str> = segs[1..].iter().map(|s| s.0.as_str()).collect();
                    Some(vec![self.edit(
                        segs[0].1.start(),
                        segs.last().unwrap().1.end(),
                        rest.join("::"),
                    )])
                } else {
                    rule.rename
                        .as_ref()
                        .filter(|_| seg.0 == *last)
                        .map(|new| vec![self.replace(&seg.1, new.clone())])
                };
            }
            self.hit(i, seg.1.start(), seg.1.end(), edits, None, None);
        }
    }

    fn check_named(&mut self, name: &str, at: Span, kind: Kind) {
        for i in self.indices(|r| r.kind == kind && r.names.iter().any(|n| n == name)) {
            let rule = self.rules[i].rule;
            if self.import_ok(rule) {
                let new = rule
                    .renames
                    .get(name)
                    .or(rule.rename.as_ref())
                    .filter(|n| *n != name)
                    .cloned();
                let edits = new.map(|n| vec![self.replace(&at, n)]);
                self.hit(i, at.start(), at.end(), edits, None, None);
            }
        }
    }

    /// `replace_call` rules: rewrite a zero-argument call of a matching path.
    fn check_call_replacements(&mut self, c: &syn::ExprCall) {
        let Expr::Path(p) = &*c.func else { return };
        if !c.args.is_empty() {
            return;
        }
        let segs = path_segs(&p.path);
        let Some((resolved, offset)) = self.resolve(&segs, p.path.leading_colon.is_some()) else {
            return;
        };
        for i in self.indices(|r| r.kind == Kind::Path && r.replace_call.is_some()) {
            let rule = self.rules[i].rule;
            let Some(pat) = rule
                .segs
                .iter()
                .find(|pat| resolved.starts_with(pat) && resolved.len() == pat.len())
            else {
                continue;
            };
            let Some(seg) = usize::try_from(pat.len() as isize - 1 - offset)
                .ok()
                .and_then(|t| segs.get(t))
            else {
                continue;
            };
            let edit = self.replace(c, rule.replace_call.clone().unwrap());
            self.pending
                .insert((i, seg.1.start().line, seg.1.start().column), vec![edit]);
        }
    }

    fn feature_gates(&mut self, attr: &syn::Attribute) {
        let mut names = Vec::new();
        let _ = attr.parse_nested_meta(|m| {
            if let Some(id) = m.path.get_ident() {
                names.push((id.to_string(), id.span()));
            }
            Ok(())
        });
        let msrv_str = self.pkg.rust_version_str();
        let stable: Vec<(String, Span, String, &str, rules::Applies)> = names
            .iter()
            .filter_map(|(name, span)| {
                let (since, area) = FEATURES.since(name)?;
                let avail = rules::availability(
                    since,
                    self.rustc,
                    self.pkg.rust_version.as_ref(),
                    &msrv_str,
                )?;
                Some((name.clone(), *span, since.to_owned(), area, avail))
            })
            .collect();
        // One rewrite for the whole attribute: drop the stable names (or the attribute).
        let removable: Vec<&str> = stable
            .iter()
            .filter(|s| !s.4.beta)
            .map(|s| s.0.as_str())
            .collect();
        let keep: Vec<&str> = names
            .iter()
            .map(|n| n.0.as_str())
            .filter(|n| !removable.contains(n))
            .collect();
        let attr_edit = (!removable.is_empty()).then(|| {
            if keep.is_empty() {
                self.delete_lines(attr.span().start(), attr.span().end())
            } else {
                self.replace(attr, format!("#![feature({})]", keep.join(", ")))
            }
        });
        for i in self.indices(|r| r.kind == Kind::FeatureGate) {
            for (k, (name, span, since, area, avail)) in stable.iter().enumerate() {
                let detail = format!("`{name}` ({area} feature) is stable since Rust {since}");
                // Attach the attribute rewrite to the first removable gate only.
                let edits = (!avail.beta
                    && removable.first() == Some(&name.as_str())
                    && k == stable.iter().position(|s| !s.4.beta).unwrap_or(usize::MAX))
                .then(|| attr_edit.clone().map(|e| vec![e]))
                .flatten();
                self.hit(
                    i,
                    span.start(),
                    span.end(),
                    edits,
                    Some(detail),
                    Some((avail.note.clone(), avail.beta)),
                );
            }
        }
    }
}

pub(crate) fn path_segs(p: &syn::Path) -> Vec<Seg> {
    p.segments
        .iter()
        .map(|s| (s.ident.unraw().to_string(), s.ident.span()))
        .collect()
}

impl<'ast> Visit<'ast> for Scanner<'_> {
    fn visit_item_use(&mut self, _: &'ast syn::ItemUse) {
        // Each `use` was flattened up front; check them once, in order.
        for leaf in std::mem::take(&mut self.uses) {
            if !leaf.glob {
                self.check_path(
                    &leaf.segs,
                    leaf.leading_colon,
                    &[Kind::Path, Kind::Macro, Kind::Attribute],
                    leaf.sole_item,
                    true,
                );
            }
        }
    }

    fn visit_path(&mut self, p: &'ast syn::Path) {
        let segs = path_segs(p);
        self.check_path(&segs, p.leading_colon.is_some(), &[Kind::Path], None, false);
        // `Type::method(..)` calls count for method rules too.
        if segs.len() >= 2 {
            let last = segs.last().unwrap();
            self.check_named(&last.0.clone(), last.1, Kind::Method);
        }
        visit::visit_path(self, p);
    }

    fn visit_expr_method_call(&mut self, m: &'ast syn::ExprMethodCall) {
        let name = m.method.unraw().to_string();
        self.check_named(&name, m.method.span(), Kind::Method);
        if let Expr::Macro(mac) = &*m.receiver {
            let s = mac.span().start();
            self.receivers.insert((s.line, s.column));
        }
        self.method_patterns(m, &name);
        visit::visit_expr_method_call(self, m);
    }

    fn visit_expr_call(&mut self, c: &'ast syn::ExprCall) {
        self.check_call_replacements(c);
        self.call_patterns(c);
        visit::visit_expr_call(self, c);
    }

    fn visit_local(&mut self, l: &'ast syn::Local) {
        self.local_patterns(l);
        visit::visit_local(self, l);
    }

    fn visit_expr_match(&mut self, m: &'ast syn::ExprMatch) {
        self.match_patterns(m);
        visit::visit_expr_match(self, m);
    }

    fn visit_expr_if(&mut self, i: &'ast syn::ExprIf) {
        self.if_patterns(i);
        visit::visit_expr_if(self, i);
    }

    fn visit_expr_assign(&mut self, a: &'ast syn::ExprAssign) {
        self.assign_patterns(a);
        visit::visit_expr_assign(self, a);
    }

    fn visit_item_impl(&mut self, i: &'ast syn::ItemImpl) {
        self.impl_patterns(i);
        visit::visit_item_impl(self, i);
    }

    fn visit_item_enum(&mut self, e: &'ast syn::ItemEnum) {
        self.enum_patterns(e);
        visit::visit_item_enum(self, e);
    }

    fn visit_item_trait(&mut self, t: &'ast syn::ItemTrait) {
        for item in &t.items {
            if let syn::TraitItem::Fn(f) = item
                && matches!(f.sig.ident.to_string().as_str(), "as_any" | "as_any_mut")
            {
                self.pattern("trait-upcasting", f.sig.ident.span(), None);
            }
        }
        visit::visit_item_trait(self, t);
    }

    fn visit_macro(&mut self, mac: &'ast syn::Macro) {
        let segs = path_segs(&mac.path);
        if let Some(last) = segs.last() {
            self.check_named(&last.0.clone(), last.1, Kind::Macro);
        }
        self.check_path(
            &segs,
            mac.path.leading_colon.is_some(),
            &[Kind::Macro],
            None,
            false,
        );
        self.macro_patterns(mac, segs.last().map(|s| s.0.as_str()).unwrap_or(""));
        visit::visit_macro(self, mac);
        // syn leaves macro bodies as tokens; look inside the common shapes.
        if let Ok(args) = mac.parse_body_with(Punctuated::<Expr, syn::Token![,]>::parse_terminated)
        {
            args.iter().for_each(|e| self.visit_expr(e));
        } else if let Ok(stmts) = mac.parse_body_with(syn::Block::parse_within) {
            stmts.iter().for_each(|s| self.visit_stmt(s));
        }
    }

    fn visit_attribute(&mut self, attr: &'ast syn::Attribute) {
        let segs = path_segs(attr.path());
        if let Some(last) = segs.last() {
            self.check_named(&last.0.clone(), last.1, Kind::Attribute);
        }
        self.check_path(&segs, false, &[Kind::Attribute], None, false);
        if attr.path().is_ident("derive") {
            let mut derived = Vec::new();
            let _ = attr.parse_nested_meta(|m| {
                derived.push(m.path.clone());
                Ok(())
            });
            for p in &derived {
                self.check_path(
                    &path_segs(p),
                    p.leading_colon.is_some(),
                    &[Kind::Path],
                    None,
                    false,
                );
            }
        }
        if matches!(attr.style, syn::AttrStyle::Inner(_)) && attr.path().is_ident("feature") {
            self.feature_gates(attr);
        }
        visit::visit_attribute(self, attr);
    }

    fn visit_item_extern_crate(&mut self, ec: &'ast syn::ItemExternCrate) {
        let name = ec.ident.unraw().to_string();
        // Removable when nothing depends on the item itself: no `#[macro_use]`, no `as` rename.
        let removable =
            ec.attrs.iter().all(|a| !a.path().is_ident("macro_use")) && ec.rename.is_none();
        let edits = removable.then(|| vec![self.delete_lines(ec.span().start(), ec.span().end())]);
        for i in self.indices(|r| r.kind == Kind::ExternCrate && !r.exclude.contains(&name)) {
            self.hit(
                i,
                ec.extern_token.span().start(),
                ec.semi_token.span().end(),
                edits.clone(),
                None,
                None,
            );
        }
        visit::visit_item_extern_crate(self, ec);
    }
}

/// Is `e` a plain identifier, `a.b`, a call, or another form that binds tighter than
/// any binary operator (safe to use as a method receiver without parentheses)?
pub(crate) fn is_tight(e: &Expr) -> bool {
    matches!(
        e,
        Expr::Path(_)
            | Expr::MethodCall(_)
            | Expr::Call(_)
            | Expr::Field(_)
            | Expr::Paren(_)
            | Expr::Lit(_)
            | Expr::Index(_)
            | Expr::Macro(_)
            | Expr::Tuple(_)
    )
}

pub(crate) fn is_int(e: &Expr, n: u64) -> bool {
    matches!(e, Expr::Lit(l) if matches!(&l.lit, Lit::Int(i) if i.base10_parse::<u64>().ok() == Some(n)))
}
