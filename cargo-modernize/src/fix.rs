//! Applying fixes, `cargo fix`-style: refuse a dirty tree, rewrite in passes until
//! nothing more applies, rustfmt what was formatted before, then `cargo check` and
//! revert any file whose rewrites don't compile.

use crate::scan::{Edit, Fix, PackageReport, line_starts};
use anyhow::{Context, Result, bail};
use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::path::{Path, PathBuf};
use std::process::Command;

pub struct Options {
    /// Also apply fixes that can change behavior or break the build.
    pub dangerous: bool,
    /// Print what would change; write nothing.
    pub diff: bool,
    pub verify: bool,
    pub format: bool,
    pub allow_dirty: bool,
    pub allow_no_vcs: bool,
    pub manifest_path: Option<PathBuf>,
    pub ws_root: PathBuf,
    /// (package root, edition) for rustfmt.
    pub editions: Vec<(PathBuf, u16)>,
}

#[derive(Default)]
pub struct Outcome {
    pub applied: usize,
    pub changed: BTreeSet<PathBuf>,
    pub reverted: Vec<PathBuf>,
    pub formatted: usize,
    pub verified: Option<bool>,
}

pub fn wanted(reports: &[PackageReport], dangerous: bool) -> Vec<&Fix> {
    reports.iter().flat_map(|r| &r.findings).filter_map(|f| f.fix.as_ref()).filter(|f| dangerous || !f.dangerous).collect()
}

/// Applies fixes to in-memory file texts. A fix is atomic: all of its edits apply, or
/// none do (when one overlaps an earlier fix or the text no longer matches).
pub fn apply(texts: &mut HashMap<PathBuf, String>, fixes: &[&Fix]) -> Result<Vec<Edit>> {
    for f in fixes {
        for e in &f.edits {
            if !texts.contains_key(&e.path) {
                let t = std::fs::read_to_string(&e.path).with_context(|| format!("reading {}", e.path.display()))?;
                texts.insert(e.path.clone(), t);
            }
        }
    }
    let mut taken: HashMap<&Path, Vec<(usize, usize)>> = HashMap::new();
    let mut accepted: Vec<Edit> = Vec::new();
    for f in fixes {
        // An edit identical to one already accepted (the same import, the same span
        // matched by two rules) is shared, not a conflict.
        let same = |e: &Edit| accepted.iter().any(|a| a.path == e.path && a.start == e.start && a.end == e.end && a.new == e.new);
        let fresh: Vec<&Edit> = f.edits.iter().filter(|e| !same(e)).collect();
        let ok = fresh.iter().all(|e| {
            let text = &texts[&e.path];
            let overlaps = taken.get(e.path.as_path()).is_some_and(|r| r.iter().any(|&(a, b)| e.start < b && a < e.end || (e.start == e.end && a < e.start && e.start < b)));
            !overlaps && text.get(e.start..e.end) == Some(e.old.as_str())
        });
        if ok {
            for e in fresh {
                taken.entry(e.path.as_path()).or_default().push((e.start, e.end.max(e.start + 1)));
                accepted.push(e.clone());
            }
        }
    }
    let mut by_file: BTreeMap<&Path, Vec<&Edit>> = BTreeMap::new();
    for e in &accepted {
        by_file.entry(e.path.as_path()).or_default().push(e);
    }
    for (path, mut edits) in by_file {
        edits.sort_by_key(|e| std::cmp::Reverse((e.start, e.end)));
        let text = texts.get_mut(path).unwrap();
        for e in edits {
            text.replace_range(e.start..e.end, &e.new);
        }
    }
    Ok(accepted)
}

/// Rewrites files in passes (each pass re-analyzes, since one fix can expose another).
pub fn run(opts: &Options, mut analyze: impl FnMut() -> Result<Vec<PackageReport>>) -> Result<Outcome> {
    let mut out = Outcome::default();
    if opts.diff {
        let reports = analyze()?;
        let mut texts = HashMap::new();
        let accepted = apply(&mut texts, &wanted(&reports, opts.dangerous))?;
        print_diff(&opts.ws_root, &accepted);
        out.applied = accepted.len();
        out.changed = accepted.iter().map(|e| e.path.clone()).collect();
        return Ok(out);
    }
    check_vcs(opts)?;

    let baseline_ok = opts.verify && {
        eprintln!("    Checking that the project builds before modernizing…");
        let ok = cargo_check(opts)?.is_empty_ok();
        if !ok {
            eprintln!("warning: the project doesn't build as-is; skipping post-fix verification");
        }
        ok
    };

    let mut originals: HashMap<PathBuf, String> = HashMap::new();
    let mut was_formatted: HashMap<PathBuf, bool> = HashMap::new();
    for pass in 1..=8 {
        let reports = analyze()?;
        let fixes = wanted(&reports, opts.dangerous);
        if fixes.is_empty() {
            break;
        }
        let mut texts = HashMap::new();
        let accepted = apply(&mut texts, &fixes)?;
        if accepted.is_empty() {
            break;
        }
        for (path, text) in &texts {
            if !accepted.iter().any(|e| e.path == *path) {
                continue;
            }
            if !originals.contains_key(path) {
                originals.insert(path.clone(), std::fs::read_to_string(path)?);
                if opts.format && path.extension().is_some_and(|x| x == "rs") {
                    was_formatted.insert(path.clone(), rustfmt(opts, path, true));
                }
            }
            std::fs::write(path, text).with_context(|| format!("writing {}", path.display()))?;
            out.changed.insert(path.clone());
        }
        out.applied += accepted.len();
        eprintln!("    Modernized {} edit(s) in pass {pass}", accepted.len());
    }

    let mut formatted = BTreeSet::new();
    for (path, was) in &was_formatted {
        if *was && out.changed.contains(path) && rustfmt(opts, path, false) {
            formatted.insert(path.clone());
        }
    }

    if baseline_ok && !out.changed.is_empty() {
        eprintln!("    Checking the modernized code…");
        for _ in 0..4 {
            let errors = cargo_check(opts)?;
            if errors.is_empty_ok() {
                out.verified = Some(true);
                break;
            }
            // Revert the changed files the errors point at (or all of them if we can't tell).
            let mut bad: Vec<PathBuf> = errors.files.iter().filter(|f| out.changed.contains(*f)).cloned().collect();
            if bad.is_empty() {
                bad = out.changed.iter().cloned().collect();
            }
            for f in &bad {
                std::fs::write(f, &originals[f])?;
                out.changed.remove(f);
                out.reverted.push(f.clone());
            }
            out.verified = Some(false);
            if out.changed.is_empty() {
                break;
            }
        }
        if out.verified == Some(false) && !out.changed.is_empty() && cargo_check(opts)?.is_empty_ok() {
            out.verified = Some(true);
        }
    }
    out.formatted = formatted.intersection(&out.changed).count();
    Ok(out)
}

fn check_vcs(opts: &Options) -> Result<()> {
    let status = Command::new("git").arg("-C").arg(&opts.ws_root).args(["status", "--porcelain"]).output();
    match status {
        Ok(o) if o.status.success() => {
            let dirty = String::from_utf8_lossy(&o.stdout).lines().any(|l| !l.trim().is_empty());
            if dirty && !opts.allow_dirty {
                bail!(
                    "the working directory has uncommitted changes; commit or stash them first so modernizing can be reviewed and undone (or pass --allow-dirty)"
                );
            }
        }
        _ if !opts.allow_no_vcs => bail!("no git repository found; modernizing rewrites files in place, so pass --allow-no-vcs if that's intended"),
        _ => {}
    }
    Ok(())
}

struct CheckResult {
    ok: bool,
    files: BTreeSet<PathBuf>,
}

impl CheckResult {
    fn is_empty_ok(&self) -> bool {
        self.ok
    }
}

fn cargo_check(opts: &Options) -> Result<CheckResult> {
    let cargo = std::env::var("CARGO").unwrap_or_else(|_| "cargo".into());
    let mut cmd = Command::new(cargo);
    cmd.args(["check", "--all-targets", "--message-format=json", "--quiet"]);
    if let Some(m) = &opts.manifest_path {
        cmd.arg("--manifest-path").arg(m);
    }
    let out = cmd.output().context("running `cargo check`")?;
    let mut files = BTreeSet::new();
    for line in String::from_utf8_lossy(&out.stdout).lines() {
        let Ok(msg) = serde_json::from_str::<serde_json::Value>(line) else { continue };
        if msg["reason"] != "compiler-message" || msg["message"]["level"] != "error" {
            continue;
        }
        for span in msg["message"]["spans"].as_array().into_iter().flatten() {
            if let Some(f) = span["file_name"].as_str() {
                let p = Path::new(f);
                files.insert(if p.is_absolute() { p.to_path_buf() } else { opts.ws_root.join(p) });
            }
        }
    }
    Ok(CheckResult { ok: out.status.success(), files })
}

/// `check = true`: is the file already rustfmt-clean? Otherwise format it.
fn rustfmt(opts: &Options, path: &Path, check: bool) -> bool {
    let edition = opts.editions.iter().filter(|(root, _)| path.starts_with(root)).max_by_key(|(root, _)| root.as_os_str().len()).map_or(2021, |e| e.1);
    let mut cmd = Command::new(std::env::var("RUSTFMT").unwrap_or_else(|_| "rustfmt".into()));
    cmd.args(["--edition", &edition.to_string()]);
    if check {
        cmd.arg("--check");
    }
    cmd.arg(path).stdout(std::process::Stdio::null()).stderr(std::process::Stdio::null());
    cmd.status().is_ok_and(|s| s.success())
}

/// Old and new lines around a group of edits to one file: (first line number, old, new).
pub fn hunk(text: &str, edits: &[&Edit]) -> (usize, Vec<String>, Vec<String>) {
    let starts = line_starts(text);
    let start = edits.iter().map(|e| e.start).min().unwrap_or(0);
    let end = edits.iter().map(|e| e.end).max().unwrap_or(0);
    let first = starts.partition_point(|&s| s <= start);
    let last = starts.partition_point(|&s| s <= end.saturating_sub(1).max(start));
    let (a, b) = (starts[first - 1], starts.get(last).copied().unwrap_or(text.len()));
    let old = &text[a..b];
    let mut new = old.to_owned();
    let mut sorted: Vec<&&Edit> = edits.iter().collect();
    sorted.sort_by_key(|e| std::cmp::Reverse(e.start));
    for e in sorted {
        new.replace_range(e.start - a..e.end - a, &e.new);
    }
    let lines = |s: &str| s.trim_end_matches('\n').split('\n').map(str::to_owned).collect::<Vec<_>>();
    let new_lines = if new.trim().is_empty() { Vec::new() } else { lines(&new) };
    (first, lines(old), new_lines)
}

fn print_diff(ws_root: &Path, edits: &[Edit]) {
    let mut by_file: BTreeMap<&Path, Vec<&Edit>> = BTreeMap::new();
    for e in edits {
        by_file.entry(e.path.as_path()).or_default().push(e);
    }
    for (path, mut edits) in by_file {
        let Ok(text) = std::fs::read_to_string(path) else { continue };
        let rel = path.strip_prefix(ws_root).unwrap_or(path).display().to_string();
        println!("--- a/{rel}\n+++ b/{rel}");
        edits.sort_by_key(|e| e.start);
        // Group edits that touch the same lines into one hunk.
        let starts = line_starts(&text);
        let line_of = |b: usize| starts.partition_point(|&s| s <= b);
        let mut groups: Vec<Vec<&Edit>> = Vec::new();
        for e in edits {
            match groups.last_mut() {
                Some(g) if line_of(e.start) <= line_of(g.iter().map(|x| x.end).max().unwrap()) => g.push(e),
                _ => groups.push(vec![e]),
            }
        }
        let mut delta: isize = 0;
        for g in groups {
            let (line, old, new) = hunk(&text, &g);
            println!("@@ -{line},{} +{},{} @@", old.len(), line as isize + delta, new.len());
            old.iter().for_each(|l| println!("-{l}"));
            new.iter().for_each(|l| println!("+{l}"));
            delta += new.len() as isize - old.len() as isize;
        }
    }
}
