//! Static analysis: parse every source file with `syn`, resolve paths through
//! `use` imports, and match them against the active rules. Structural
//! "new ways to be Rusty" detectors live at the bottom of this file.

use crate::config::Config;
use crate::ecosystem::{self, Ecosystem};
use crate::project::{LockPkg, Package};
use crate::rules::{self, FEATURES, Kind, Level, Rule};
use anyhow::Result;
use proc_macro2::Span;
use quote::ToTokens;
use semver::Version;
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use syn::ext::IdentExt;
use syn::punctuated::Punctuated;
use syn::spanned::Spanned;
use syn::visit::{self, Visit};
use syn::{Expr, Lit, Pat, Stmt};

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
    pub line: usize,
    /// 1-based.
    pub column: usize,
    pub snippet: String,
    /// Finding-specific detail, e.g. which feature gate.
    pub detail: Option<String>,
    pub note: Option<String>,
    pub fix: Option<Edit>,
}

#[derive(Clone)]
pub struct Edit {
    pub path: PathBuf,
    pub line: usize,
    /// 0-based, in chars.
    pub col: usize,
    pub old: String,
    pub new: String,
}

struct Active<'r> {
    rule: &'r Rule,
    note: Option<String>,
    beta: bool,
}

/// A raw match before levels are resolved.
struct Hit {
    rule: usize,
    file: PathBuf,
    line: usize,
    col: usize,
    snippet: String,
    detail: Option<String>,
    note: Option<String>,
    beta: bool,
    fix: Option<Edit>,
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
        .filter(|r| r.scope.as_ref().is_none_or(|s| pkg.root.starts_with(s) || s.starts_with(&pkg.root)))
        .filter_map(|rule| rule.applies(pkg, cx.rustc).map(|a| Active { rule, note: a.note, beta: a.beta }))
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
        let Ok(text) = std::fs::read_to_string(&file) else { continue };
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
        s.visit_file(&ast);
        hits.extend(s.out);
    }

    let findings = hits
        .into_iter()
        .filter_map(|h| {
            let a = &active[h.rule];
            if a.rule.scope.as_ref().is_some_and(|s| !h.file.starts_with(s)) {
                return None;
            }
            let mut level = cx.config.level(a.rule, &h.file);
            if h.beta && level > Level::Suggest {
                level = Level::Suggest; // can't act on it with a stable toolchain yet
            }
            (level != Level::Off).then(|| Finding {
                rule: a.rule.clone(),
                level,
                file: h.file.strip_prefix(cx.ws_root).unwrap_or(&h.file).to_path_buf(),
                line: h.line,
                column: h.col + 1,
                snippet: h.snippet,
                detail: h.detail,
                note: h.note,
                fix: h.fix,
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

fn source_files(root: &Path) -> Vec<PathBuf> {
    walkdir::WalkDir::new(root)
        .into_iter()
        .filter_entry(|e| {
            let name = e.file_name().to_string_lossy();
            if e.depth() == 0 {
                return true;
            }
            // Skip build output, VCS dirs, and nested packages (they're analyzed on their own).
            !(e.file_type().is_dir() && (name == "target" || name.starts_with('.') || e.path().join("Cargo.toml").exists()))
        })
        .filter_map(|e| e.ok())
        .filter(|e| e.file_type().is_file() && e.path().extension().is_some_and(|x| x == "rs"))
        .map(|e| e.into_path())
        .collect()
}

// ---------------------------------------------------------------- Cargo.toml

fn manifest_hits(pkg: &Package, active: &[Active]) -> Vec<Hit> {
    let mut out = Vec::new();
    let text = std::fs::read_to_string(&pkg.manifest_path).unwrap_or_default();
    for (i, a) in active.iter().enumerate() {
        let hit = |file: &Path, line: usize, snippet: &str, fix| Hit {
            rule: i,
            file: file.to_path_buf(),
            line: line + 1,
            col: 0,
            snippet: snippet.trim().to_owned(),
            detail: None,
            note: a.note.clone(),
            beta: a.beta,
            fix,
        };
        match a.rule.kind {
            Kind::Dependency => {
                let dep = pkg.dep(a.rule.krate.as_deref().unwrap()).unwrap();
                let Some((mut path, mut lines, mut idx)) = locate_dep(&pkg.manifest_path, &text, dep.key()) else { continue };
                // `dep = { workspace = true }`: the version lives in the workspace manifest.
                if lines[idx].contains("workspace") && pkg.workspace_manifest != pkg.manifest_path {
                    let root_text = std::fs::read_to_string(&pkg.workspace_manifest).unwrap_or_default();
                    if let Some(found) = locate_dep(&pkg.workspace_manifest, &root_text, dep.key()) {
                        (path, lines, idx) = found;
                    }
                }
                let fix = a.rule.to_version.as_ref().and_then(|new| {
                    let (line, col, old) = version_string(&lines, idx)?;
                    (old != *new).then(|| Edit { path: path.clone(), line: line + 1, col, old, new: new.clone() })
                });
                out.push(hit(&path, idx, &lines[idx], fix));
            }
            Kind::Edition => {
                let lines: Vec<&str> = text.lines().collect();
                let idx = lines.iter().position(|l| l.trim_start().starts_with("edition")).unwrap_or(0);
                out.push(hit(&pkg.manifest_path, idx, lines.get(idx).copied().unwrap_or(""), None));
            }
            _ => {}
        }
    }
    out
}

/// Dependency findings from the ecosystem data: status of direct dependencies,
/// newer major versions, and hard problems in transitive dependencies.
fn ecosystem_hits(pkg: &Package, cx: &Context, active: &[Active]) -> Vec<Hit> {
    let rule_for = |status: &str| active.iter().position(|a| a.rule.kind == Kind::Ecosystem && a.rule.pattern.as_deref() == Some(status));
    // Hand-written dependency rules carry better advice; don't double-report those crates.
    let covered: HashSet<&str> = active.iter().filter(|a| a.rule.kind == Kind::Dependency).filter_map(|a| a.rule.krate.as_deref()).collect();
    let text = std::fs::read_to_string(&pkg.manifest_path).unwrap_or_default();
    let mut out = Vec::new();
    let mut seen = HashSet::new();
    let hit = |rule: usize, file: &Path, line: usize, snippet: &str, detail: String| Hit {
        rule,
        file: file.to_path_buf(),
        line: line + 1,
        col: 0,
        snippet: snippet.trim().to_owned(),
        detail: Some(detail),
        note: active[rule].note.clone(),
        beta: false,
        fix: None,
    };

    for dep in &pkg.deps {
        if covered.contains(dep.name.as_str()) || !seen.insert(dep.name.as_str()) {
            continue;
        }
        let Some(entry) = cx.ecosystem.crates.get(&dep.name) else { continue };
        let Some((path, lines, idx)) = locate_dep(&pkg.manifest_path, &text, dep.key()) else { continue };
        if let Some(status) = entry.status.as_deref() {
            if let Some(r) = rule_for(status) {
                out.push(hit(r, &path, idx, &lines[idx], entry.advice()));
            }
            continue;
        }
        // Newer semver-incompatible release?
        let (Some(current), Some(latest)) = (dep.version(), entry.latest.as_deref().and_then(|l| Version::parse(l).ok())) else { continue };
        let compatible = semver::VersionReq::parse(&format!("^{current}")).is_ok_and(|req| req.matches(&latest));
        if latest > current && latest.pre.is_empty() && !compatible {
            if let Some(r) = rule_for("outdated") {
                out.push(hit(r, &path, idx, &lines[idx], format!("On {current}; the latest release is {latest}.")));
            }
        }
    }

    // Transitive: walk Cargo.lock from this package.
    let (Some(r), Some(root)) = (rule_for("transitive"), cx.lock.iter().find(|l| l.name == pkg.name && l.version == pkg.version)) else {
        return out;
    };
    let lock_path = cx.ws_root.join("Cargo.lock");
    let lock_text = std::fs::read_to_string(&lock_path).unwrap_or_default();
    let lock_lines: Vec<&str> = lock_text.lines().collect();
    let find = |spec: &str| -> Option<&LockPkg> {
        let mut parts = spec.split(' ');
        let (name, version) = (parts.next()?, parts.next());
        cx.lock.iter().find(|l| l.name == name && version.is_none_or(|v| l.version == v))
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
        if direct.contains(name) || name == pkg.name || covered.contains(name) || !reported.insert(name) {
            continue;
        }
        let Some(entry) = cx.ecosystem.crates.get(name) else { continue };
        let Some(status) = entry.status.as_deref().filter(|s| ecosystem::HARD.contains(s)) else { continue };
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
        let line = lock_lines.iter().position(|l| *l == format!("name = \"{name}\"")).unwrap_or(0);
        let detail = format!("`{name}` is {status}; pulled in via {}. {}", chain.join(" → "), entry.advice());
        out.push(hit(r, &lock_path, line, lock_lines.get(line).copied().unwrap_or(""), detail));
    }
    out.sort_by_key(|h| (h.file.clone(), h.line));
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
            let k = l.split(['=', '.']).next().unwrap_or("").trim().trim_matches('"');
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
        None if line[line.find('=')?..].trim_start_matches('=').trim_start().starts_with('"') => line.find('=')?,
        None => return None,
    };
    quoted_after(line, from).map(|(c, s)| (idx, c, s))
}

// ---------------------------------------------------------------- source

type Seg = (String, Span);

struct UseLeaf {
    segs: Vec<Seg>,
    alias: Option<String>,
    glob: bool,
    leading_colon: bool,
}

#[derive(Default)]
struct UseCollector {
    leaves: Vec<UseLeaf>,
}

impl<'ast> Visit<'ast> for UseCollector {
    fn visit_item_use(&mut self, u: &'ast syn::ItemUse) {
        flatten(&u.tree, Vec::new(), u.leading_colon.is_some(), &mut self.leaves);
    }
}

fn flatten(tree: &syn::UseTree, mut prefix: Vec<Seg>, lc: bool, out: &mut Vec<UseLeaf>) {
    use syn::UseTree::*;
    match tree {
        Path(p) => {
            prefix.push((p.ident.unraw().to_string(), p.ident.span()));
            flatten(&p.tree, prefix, lc, out);
        }
        Name(n) if n.ident == "self" => {
            let alias = prefix.last().map(|s| s.0.clone());
            out.push(UseLeaf { segs: prefix, alias, glob: false, leading_colon: lc });
        }
        Name(n) => {
            prefix.push((n.ident.unraw().to_string(), n.ident.span()));
            let alias = Some(n.ident.unraw().to_string());
            out.push(UseLeaf { segs: prefix, alias, glob: false, leading_colon: lc });
        }
        Rename(r) => {
            prefix.push((r.ident.unraw().to_string(), r.ident.span()));
            let alias = Some(r.rename.unraw().to_string()).filter(|a| a != "_");
            out.push(UseLeaf { segs: prefix, alias, glob: false, leading_colon: lc });
        }
        Glob(_) => out.push(UseLeaf { segs: prefix, alias: None, glob: true, leading_colon: lc }),
        Group(g) => g.items.iter().for_each(|t| flatten(t, prefix.clone(), lc, out)),
    }
}

struct Scanner<'a> {
    rules: &'a [Active<'a>],
    root_map: &'a HashMap<String, String>,
    aliases: HashMap<String, Vec<String>>,
    /// Fully-resolved import paths (glob imports contribute their prefix).
    imports: Vec<Vec<String>>,
    uses: Vec<UseLeaf>,
    path: &'a Path,
    lines: Vec<&'a str>,
    pkg: &'a Package,
    rustc: Option<&'a Version>,
    out: Vec<Hit>,
    seen: HashSet<(String, usize, usize)>,
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
            lines: text.lines().collect(),
            pkg,
            rustc,
            out: Vec::new(),
            seen: HashSet::new(),
        };
        for leaf in &uses {
            let names: Vec<String> = leaf.segs.iter().map(|s| s.0.clone()).collect();
            if names.first().is_some_and(|f| matches!(f.as_str(), "crate" | "self" | "super")) {
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
            Some(full) => Some((full.iter().cloned().chain(rest).collect(), full.len() as isize - 1)),
            None => Some((self.map_root(segs.iter().map(|s| s.0.clone()).collect()), 0)),
        }
    }

    fn resolve_expr_path(&self, e: &Expr) -> Option<Vec<String>> {
        let Expr::Path(p) = e else { return None };
        self.resolve(&path_segs(&p.path), p.path.leading_colon.is_some()).map(|(r, _)| r)
    }

    fn import_ok(&self, rule: &Rule) -> bool {
        rule.import_segs.is_empty()
            || rule.import_segs.iter().any(|req| self.imports.iter().any(|imp| imp.starts_with(req)))
    }

    fn hit(&mut self, idx: usize, at: Span, fix: Option<(Span, String)>, detail: Option<String>, note: Option<(Option<String>, bool)>) {
        let a = &self.rules[idx];
        let start = at.start();
        if !self.seen.insert((a.rule.id.clone() + detail.as_deref().unwrap_or(""), start.line, start.column)) {
            return;
        }
        let fix = match (&a.rule.rename, fix) {
            (Some(new), Some((sp, old))) if old != *new => {
                Some(Edit { path: self.path.to_path_buf(), line: sp.start().line, col: sp.start().column, old, new: new.clone() })
            }
            _ => None,
        };
        let (note, beta) = note.unwrap_or((a.note.clone(), a.beta));
        self.out.push(Hit {
            rule: idx,
            file: self.path.to_path_buf(),
            line: start.line,
            col: start.column,
            snippet: self.lines.get(start.line.wrapping_sub(1)).map_or(String::new(), |l| l.trim().to_owned()),
            detail,
            note,
            beta,
            fix,
        });
    }

    fn indices(&self, pred: impl Fn(&Rule) -> bool) -> Vec<usize> {
        self.rules.iter().enumerate().filter(|(_, a)| pred(a.rule)).map(|(i, _)| i).collect()
    }

    /// Matches a path against pattern-based rules of the given kinds.
    fn check_path(&mut self, segs: &[Seg], leading_colon: bool, kinds: &[Kind]) {
        let Some((resolved, offset)) = self.resolve(segs, leading_colon) else { return };
        for i in self.indices(|r| kinds.contains(&r.kind)) {
            let rule = self.rules[i].rule;
            let Some(pat) = rule.segs.iter().find(|p| resolved.starts_with(p)) else { continue };
            // The code segment the rule's last component lines up with. If it's hidden
            // inside an import alias, the `use` itself is reported (and fixed) instead.
            let Some(seg) = usize::try_from(pat.len() as isize - 1 - offset).ok().and_then(|i| segs.get(i)) else {
                continue;
            };
            let fix = (seg.0 == *pat.last().unwrap()).then(|| (seg.1, seg.0.clone()));
            self.hit(i, seg.1, fix, None, None);
        }
    }

    fn check_named(&mut self, name: &str, at: Span, kind: Kind) {
        for i in self.indices(|r| r.kind == kind && r.names.iter().any(|n| n == name)) {
            if self.import_ok(self.rules[i].rule) {
                self.hit(i, at, Some((at, name.to_owned())), None, None);
            }
        }
    }

    /// Reports a built-in structural pattern.
    fn pattern(&mut self, name: &str, at: Span) {
        for i in self.indices(|r| r.kind == Kind::Pattern && r.pattern.as_deref() == Some(name)) {
            if self.import_ok(self.rules[i].rule) {
                self.hit(i, at, None, None, None);
            }
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
        for i in self.indices(|r| r.kind == Kind::FeatureGate) {
            for (name, span) in &names {
                let Some((since, area)) = FEATURES.since(name) else { continue };
                let Some(avail) = rules::availability(since, self.rustc, self.pkg.rust_version.as_ref(), &msrv_str) else { continue };
                let detail = format!("`{name}` ({area} feature) is stable since Rust {since}");
                self.hit(i, *span, None, Some(detail), Some((avail.note, avail.beta)));
            }
        }
    }
}

fn path_segs(p: &syn::Path) -> Vec<Seg> {
    p.segments.iter().map(|s| (s.ident.unraw().to_string(), s.ident.span())).collect()
}

impl<'ast> Visit<'ast> for Scanner<'_> {
    fn visit_item_use(&mut self, _: &'ast syn::ItemUse) {
        // Each `use` was flattened up front; check them once, in order.
        for leaf in std::mem::take(&mut self.uses) {
            if !leaf.glob {
                self.check_path(&leaf.segs, leaf.leading_colon, &[Kind::Path, Kind::Macro, Kind::Attribute]);
            }
        }
    }

    fn visit_path(&mut self, p: &'ast syn::Path) {
        let segs = path_segs(p);
        self.check_path(&segs, p.leading_colon.is_some(), &[Kind::Path]);
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
        let at = m.method.span();
        match (name.as_str(), m.args.len()) {
            ("map_or", 2) => match &m.args[0] {
                Expr::Lit(l) if matches!(&l.lit, Lit::Bool(b) if !b.value) => self.pattern("is-some-and", at),
                Expr::Lit(l) if matches!(&l.lit, Lit::Bool(b) if b.value) => self.pattern("is-none-or", at),
                _ => {}
            },
            ("splitn", 2) if is_int(&m.args[0], 2) => self.pattern("split-once", at),
            ("map", 1) if is_as_ref_map(m) => self.pattern("as-deref", at),
            ("unwrap_or", 1) => {
                if let Expr::MethodCall(inner) = &*m.receiver {
                    if matches!(inner.method.to_string().as_str(), "strip_prefix" | "strip_suffix") && tokens(&m.args[0]) == tokens(&inner.receiver) {
                        self.pattern("trim-prefix", at);
                    }
                }
            }
            ("into_owned" | "to_string", 0) => {
                if let Expr::Call(c) = &*m.receiver {
                    if path_ends_with(&c.func, &["String", "from_utf8_lossy"]) {
                        self.pattern("lossy-owned", at);
                    }
                }
            }
            ("collect", 0) if is_zero_range_map(&m.receiver) && collects_vec_or_unknown(m) => self.pattern("vec-from-fn", at),
            ("with", 1) if is_cell_set_get(&m.args[0]) => self.pattern("local-key-update", at),
            _ => {}
        }
        visit::visit_expr_method_call(self, m);
    }

    fn visit_expr_call(&mut self, c: &'ast syn::ExprCall) {
        if let Some(f) = self.resolve_expr_path(&c.func) {
            let f: Vec<&str> = f.iter().map(String::as_str).collect();
            if matches!(f.as_slice(), ["std", "io", "Error", "new"]) && c.args.first().is_some_and(|a| path_ends_with(a, &["Other"])) {
                self.pattern("io-error-other", c.func.span());
            }
            if matches!(f.as_slice(), ["std" | "core", "mem", "replace"]) && c.args.len() == 2 && is_default_value(&c.args[1]) {
                self.pattern("mem-take", c.func.span());
            }
        }
        visit::visit_expr_call(self, c);
    }

    fn visit_local(&mut self, l: &'ast syn::Local) {
        if let Some(init) = l.init.as_ref().filter(|i| i.diverge.is_none()) {
            let let_else = match &*init.expr {
                Expr::Match(m) => is_unwrap_or_diverge_match(m),
                Expr::If(i) => is_unwrap_or_diverge_if(i),
                _ => false,
            };
            if let_else {
                self.pattern("let-else", l.let_token.span);
            }
        }
        visit::visit_local(self, l);
    }

    fn visit_expr_match(&mut self, m: &'ast syn::ExprMatch) {
        let bools: Vec<bool> = m.arms.iter().filter_map(|a| bool_lit(&a.body)).collect();
        if m.arms.len() >= 2
            && bools.len() == m.arms.len()
            && matches!(m.arms.last().unwrap().pat, Pat::Wild(_))
            && bools.iter().any(|b| *b != bools[0])
        {
            self.pattern("matches-macro", m.match_token.span);
        }
        visit::visit_expr_match(self, m);
    }

    fn visit_expr_if(&mut self, i: &'ast syn::ExprIf) {
        if let Some((_, els)) = &i.else_branch {
            if single_expr(&i.then_branch).is_some_and(is_ok_unit) && block_expr(els).is_some_and(|e| call_named(e, "Err")) {
                self.pattern("bool-ok-or", i.if_token.span);
            }
        } else if matches!(*i.cond, Expr::Let(_)) {
            if let [Stmt::Expr(Expr::If(inner), _)] = i.then_branch.stmts.as_slice() {
                if inner.else_branch.is_none() {
                    self.pattern("let-chains", i.if_token.span);
                }
            }
        }
        visit::visit_expr_if(self, i);
    }

    fn visit_expr_assign(&mut self, a: &'ast syn::ExprAssign) {
        if let Expr::Unary(u) = &*a.right {
            if matches!(u.op, syn::UnOp::Not(_)) && tokens(&u.expr) == tokens(&a.left) {
                self.pattern("bool-toggle", a.eq_token.span);
            }
        }
        visit::visit_expr_assign(self, a);
    }

    fn visit_item_impl(&mut self, i: &'ast syn::ItemImpl) {
        let is_default = i.trait_.as_ref().is_some_and(|(_, p, _)| p.segments.last().is_some_and(|s| s.ident == "Default"));
        if is_default {
            if let [syn::ImplItem::Fn(f)] = i.items.as_slice() {
                if matches!(single_expr(&f.block), Some(Expr::Path(p)) if p.path.segments.len() >= 2) {
                    self.pattern("derive-default-enum", i.impl_token.span);
                }
            }
        }
        visit::visit_item_impl(self, i);
    }

    fn visit_item_enum(&mut self, e: &'ast syn::ItemEnum) {
        for v in &e.variants {
            if v.ident.to_string().starts_with("__") {
                self.pattern("non-exhaustive", v.ident.span());
            }
        }
        visit::visit_item_enum(self, e);
    }

    fn visit_item_trait(&mut self, t: &'ast syn::ItemTrait) {
        for item in &t.items {
            if let syn::TraitItem::Fn(f) = item {
                if matches!(f.sig.ident.to_string().as_str(), "as_any" | "as_any_mut") {
                    self.pattern("trait-upcasting", f.sig.ident.span());
                }
            }
        }
        visit::visit_item_trait(self, t);
    }

    fn visit_macro(&mut self, mac: &'ast syn::Macro) {
        let segs = path_segs(&mac.path);
        if let Some(last) = segs.last() {
            self.check_named(&last.0.clone(), last.1, Kind::Macro);
        }
        self.check_path(&segs, mac.path.leading_colon.is_some(), &[Kind::Macro]);
        visit::visit_macro(self, mac);
        // syn leaves macro bodies as tokens; look inside the common shapes.
        if let Ok(args) = mac.parse_body_with(Punctuated::<Expr, syn::Token![,]>::parse_terminated) {
            if let Some(name) = segs.last().map(|s| s.0.as_str()) {
                if has_uninlined_format_args(name, &args) {
                    self.pattern("inline-format-args", mac.path.span());
                }
            }
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
        self.check_path(&segs, false, &[Kind::Attribute]);
        if attr.path().is_ident("derive") {
            let mut derived = Vec::new();
            let _ = attr.parse_nested_meta(|m| {
                derived.push(m.path.clone());
                Ok(())
            });
            for p in &derived {
                self.check_path(&path_segs(p), p.leading_colon.is_some(), &[Kind::Path]);
            }
        }
        if matches!(attr.style, syn::AttrStyle::Inner(_)) && attr.path().is_ident("feature") {
            self.feature_gates(attr);
        }
        visit::visit_attribute(self, attr);
    }

    fn visit_item_extern_crate(&mut self, ec: &'ast syn::ItemExternCrate) {
        let name = ec.ident.unraw().to_string();
        for i in self.indices(|r| r.kind == Kind::ExternCrate && !r.exclude.contains(&name)) {
            self.hit(i, ec.extern_token.span(), None, None, None);
        }
        visit::visit_item_extern_crate(self, ec);
    }
}

// ---------------------------------------------------------------- pattern helpers

fn tokens(e: &impl ToTokens) -> String {
    e.to_token_stream().to_string()
}

fn strip_parens(e: &Expr) -> &Expr {
    match e {
        Expr::Paren(p) => strip_parens(&p.expr),
        e => e,
    }
}

fn is_int(e: &Expr, n: u64) -> bool {
    matches!(e, Expr::Lit(l) if matches!(&l.lit, Lit::Int(i) if i.base10_parse::<u64>().ok() == Some(n)))
}

fn bool_lit(e: &Expr) -> Option<bool> {
    match e {
        Expr::Lit(l) => match &l.lit {
            Lit::Bool(b) => Some(b.value),
            _ => None,
        },
        Expr::Block(b) => single_expr(&b.block).and_then(bool_lit),
        _ => None,
    }
}

fn path_ends_with(e: &Expr, tail: &[&str]) -> bool {
    let Expr::Path(p) = e else { return false };
    let segs: Vec<String> = p.path.segments.iter().map(|s| s.ident.to_string()).collect();
    segs.len() >= tail.len() && segs[segs.len() - tail.len()..].iter().zip(tail).all(|(a, b)| a == b)
}

/// The block's only content, if it is a single tail expression.
fn single_expr(b: &syn::Block) -> Option<&Expr> {
    match b.stmts.as_slice() {
        [Stmt::Expr(e, None)] => Some(e),
        _ => None,
    }
}

fn block_expr(e: &Expr) -> Option<&Expr> {
    match e {
        Expr::Block(b) => single_expr(&b.block),
        _ => None,
    }
}

fn call_named(e: &Expr, name: &str) -> bool {
    matches!(e, Expr::Call(c) if c.args.len() == 1 && path_ends_with(&c.func, &[name]))
}

fn is_ok_unit(e: &Expr) -> bool {
    matches!(e, Expr::Call(c) if path_ends_with(&c.func, &["Ok"]) && matches!(c.args.first(), Some(Expr::Tuple(t)) if t.elems.is_empty()))
}

fn is_diverging(e: &Expr) -> bool {
    match e {
        Expr::Return(_) | Expr::Continue(_) | Expr::Break(_) => true,
        Expr::Macro(m) => is_diverging_macro(&m.mac),
        Expr::Block(b) => block_diverges(&b.block),
        _ => false,
    }
}

fn is_diverging_macro(m: &syn::Macro) -> bool {
    m.path.segments.last().is_some_and(|s| matches!(s.ident.to_string().as_str(), "panic" | "unreachable" | "todo" | "unimplemented" | "bail"))
}

fn block_diverges(b: &syn::Block) -> bool {
    match b.stmts.last() {
        Some(Stmt::Expr(e, _)) => is_diverging(e),
        Some(Stmt::Macro(m)) => is_diverging_macro(&m.mac),
        _ => false,
    }
}

fn bindings(p: &Pat, out: &mut Vec<String>) {
    match p {
        // `None`, `FOO`: syn can't tell a unit variant/constant from a binding; go by case.
        Pat::Ident(i) if i.subpat.is_none() && i.ident.to_string().starts_with(char::is_uppercase) => {}
        Pat::Ident(i) => {
            out.push(i.ident.to_string());
            if let Some((_, sub)) = &i.subpat {
                bindings(sub, out);
            }
        }
        Pat::TupleStruct(t) => t.elems.iter().for_each(|e| bindings(e, out)),
        Pat::Tuple(t) => t.elems.iter().for_each(|e| bindings(e, out)),
        Pat::Struct(s) => s.fields.iter().for_each(|f| bindings(&f.pat, out)),
        Pat::Reference(r) => bindings(&r.pat, out),
        _ => {}
    }
}

/// The arm/branch produces exactly the one variable its pattern binds.
fn yields_binding(pat: &Pat, body: &Expr) -> bool {
    let mut names = Vec::new();
    bindings(pat, &mut names);
    let body = block_expr(body).unwrap_or(body);
    matches!((names.as_slice(), body), ([n], Expr::Path(p)) if p.path.is_ident(n))
}

/// The `else` of a let-else can't see anything the failing pattern binds.
fn binds_nothing(p: &Pat) -> bool {
    let mut names = Vec::new();
    bindings(p, &mut names);
    names.is_empty()
}

/// `let x = match e { Some(x) => x, None => return };`
/// Not `Err(e) => return Err(e.into())`: let-else's `else` can't use `e`.
fn is_unwrap_or_diverge_match(m: &syn::ExprMatch) -> bool {
    let [a, b] = m.arms.as_slice() else { return false };
    if a.guard.is_some() || b.guard.is_some() {
        return false;
    }
    let ok = |keep: &syn::Arm, bail: &syn::Arm| yields_binding(&keep.pat, &keep.body) && binds_nothing(&bail.pat) && is_diverging(&bail.body);
    ok(a, b) || ok(b, a)
}

/// `let x = if let Some(x) = e { x } else { return };`
fn is_unwrap_or_diverge_if(i: &syn::ExprIf) -> bool {
    let (Expr::Let(l), Some((_, els))) = (&*i.cond, &i.else_branch) else { return false };
    single_expr(&i.then_branch).is_some_and(|e| yields_binding(&l.pat, e)) && is_diverging(els)
}

/// `x.as_ref().map(|s| s.as_str())` or `.map(String::as_str)`
fn is_as_ref_map(m: &syn::ExprMethodCall) -> bool {
    let Expr::MethodCall(inner) = &*m.receiver else { return false };
    if inner.method != "as_ref" || !inner.args.is_empty() {
        return false;
    }
    const DEREFS: [&str; 4] = ["as_str", "as_slice", "as_path", "as_os_str"];
    match &m.args[0] {
        Expr::Path(p) => p.path.segments.last().is_some_and(|s| DEREFS.contains(&s.ident.to_string().as_str())),
        Expr::Closure(c) => match (c.inputs.first(), strip_parens(&c.body)) {
            (Some(Pat::Ident(param)), Expr::MethodCall(call)) => {
                call.args.is_empty() && DEREFS.contains(&call.method.to_string().as_str()) && matches!(&*call.receiver, Expr::Path(p) if p.path.is_ident(&param.ident))
            }
            _ => false,
        },
        _ => false,
    }
}

/// `(0..n).map(f).collect()`
fn is_zero_range_map(e: &Expr) -> bool {
    let Expr::MethodCall(map) = e else { return false };
    map.method == "map"
        && matches!(strip_parens(&map.receiver), Expr::Range(r)
            if matches!(r.limits, syn::RangeLimits::HalfOpen(_)) && r.end.is_some() && r.start.as_deref().is_some_and(|s| is_int(s, 0)))
}

/// `collect()` or `collect::<Vec<_>>()`, not `collect::<String>()` etc.
fn collects_vec_or_unknown(m: &syn::ExprMethodCall) -> bool {
    m.turbofish.as_ref().is_none_or(|t| {
        matches!(t.args.first(), Some(syn::GenericArgument::Type(syn::Type::Path(p))) if p.path.segments.last().is_some_and(|s| s.ident == "Vec"))
    })
}

/// `KEY.with(|c| c.set(c.get() + 1))`
fn is_cell_set_get(e: &Expr) -> bool {
    let Expr::Closure(c) = e else { return false };
    let (Some(Pat::Ident(p)), body) = (c.inputs.first(), strip_parens(&c.body)) else { return false };
    let body = block_expr(body).unwrap_or(body);
    let Expr::MethodCall(set) = body else { return false };
    set.method == "set"
        && matches!(&*set.receiver, Expr::Path(r) if r.path.is_ident(&p.ident))
        && set.args.first().is_some_and(|a| tokens(a).contains(&format!("{} . get ()", p.ident)))
}

/// `Default::default()`, `T::default()`, `Vec::new()`, `String::new()`, …
fn is_default_value(e: &Expr) -> bool {
    const EMPTY: [&str; 7] = ["Vec", "String", "HashMap", "HashSet", "BTreeMap", "BTreeSet", "VecDeque"];
    let Expr::Call(c) = e else { return false };
    let Expr::Path(p) = &*c.func else { return false };
    let segs: Vec<String> = p.path.segments.iter().map(|s| s.ident.to_string()).collect();
    c.args.is_empty()
        && match segs.as_slice() {
            [.., last] if last == "default" => true,
            [.., ty, last] if last == "new" => EMPTY.contains(&ty.as_str()),
            _ => false,
        }
}

/// `println!("{}", x)` where an implicit `{}` is fed a plain identifier.
fn has_uninlined_format_args(mac: &str, args: &Punctuated<Expr, syn::Token![,]>) -> bool {
    let fmt_index = match mac {
        "write" | "writeln" => 1,
        "format" | "print" | "println" | "eprint" | "eprintln" | "panic" | "format_args" | "unreachable" | "todo"
        | "unimplemented" | "bail" | "anyhow" | "info" | "warn" | "error" | "debug" | "trace" => 0,
        _ => return false,
    };
    let Some(Expr::Lit(syn::ExprLit { lit: Lit::Str(s), .. })) = args.iter().nth(fmt_index) else { return false };
    let fmt = s.value();
    let mut implicit = 0;
    let mut chars = fmt.chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            '{' if chars.peek() == Some(&'{') => {
                chars.next();
            }
            '{' => {
                let spec: String = chars.by_ref().take_while(|&c| c != '}').collect();
                if spec.is_empty() || spec.starts_with(':') {
                    implicit += 1;
                } else {
                    return false; // already uses named/positional arguments
                }
            }
            _ => {}
        }
    }
    let rest: Vec<&Expr> = args.iter().skip(fmt_index + 1).collect();
    implicit > 0
        && rest.len() == implicit
        && rest.iter().any(|e| matches!(e, Expr::Path(p) if p.qself.is_none() && p.path.get_ident().is_some()))
}
