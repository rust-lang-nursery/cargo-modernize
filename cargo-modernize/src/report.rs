use crate::rules::{Category, Level, Rule};
use crate::fix::hunk;
use crate::scan::{Finding, PackageReport};
use serde_json::json;
use std::collections::{BTreeMap, HashMap};
use std::io::IsTerminal;

struct Style(bool);

impl Style {
    fn paint(&self, code: &str, s: impl std::fmt::Display) -> String {
        if self.0 { format!("\x1b[{code}m{s}\x1b[0m") } else { s.to_string() }
    }
    fn bold(&self, s: impl std::fmt::Display) -> String { self.paint("1", s) }
    fn dim(&self, s: impl std::fmt::Display) -> String { self.paint("2", s) }
    fn red(&self, s: impl std::fmt::Display) -> String { self.paint("31", s) }
    fn cyan(&self, s: impl std::fmt::Display) -> String { self.paint("36", s) }
    fn green(&self, s: impl std::fmt::Display) -> String { self.paint("32", s) }
    fn yellow(&self, s: impl std::fmt::Display) -> String { self.paint("33", s) }
    fn magenta(&self, s: impl std::fmt::Display) -> String { self.paint("35", s) }
    fn blue(&self, s: impl std::fmt::Display) -> String { self.paint("1;34", s) }

    fn level(&self, l: Level) -> String {
        match l {
            Level::Deny => self.red("deny"),
            Level::Warn => self.yellow("warn"),
            Level::Suggest => self.magenta("suggest"),
            Level::Off => self.dim("off"),
        }
    }
}

fn style() -> Style {
    Style(std::io::stdout().is_terminal() && std::env::var_os("NO_COLOR").is_none())
}

/// Upgrade plans first (by name), then the remaining categories.
fn group_key(f: &Finding) -> (u8, Category, String) {
    let title = f.rule.plan.clone().unwrap_or_else(|| f.rule.category.title().to_owned());
    (u8::from(f.rule.category != Category::Upgrade), f.rule.category, title)
}

/// How much source was looked at, across all packages.
struct Totals {
    files: usize,
    lines: usize,
    skipped_files: usize,
    skipped_lines: usize,
}

impl Totals {
    fn of(reports: &[PackageReport]) -> Self {
        Totals {
            files: reports.iter().map(|r| r.coverage.files).sum(),
            lines: reports.iter().map(|r| r.coverage.lines).sum(),
            skipped_files: reports.iter().map(|r| r.coverage.skipped.len()).sum(),
            skipped_lines: reports.iter().map(|r| r.coverage.skipped_lines()).sum(),
        }
    }

    /// Share of all source lines that were not scanned, 0 to 100.
    fn skipped_pct(&self) -> f64 {
        let all = self.lines + self.skipped_lines;
        if all == 0 { 0.0 } else { 100.0 * self.skipped_lines as f64 / all as f64 }
    }
}

/// One line saying what was and was not checked, so a clean result can't pass for a complete one.
/// `unanalyzed` are nested crates that are not workspace members.
pub fn coverage_line(reports: &[PackageReport], unanalyzed: &[String]) -> String {
    let t = Totals::of(reports);
    let mut line = format!("scanned {} file(s), {} line(s)", t.files, t.lines);
    if t.skipped_files > 0 {
        line += &format!(
            "; NOT scanned: {} file(s) that failed to parse ({} line(s), {:.1}%)",
            t.skipped_files,
            t.skipped_lines,
            t.skipped_pct()
        );
    }
    if !unanalyzed.is_empty() {
        line += &format!("; NOT analyzed: {} nested crate(s) that are not workspace members", unanalyzed.len());
    }
    line
}

/// The grouped view: one section per upgrade plan / category.
pub fn plans(reports: &[PackageReport], rustc: Option<&semver::Version>, unanalyzed: &[String]) {
    let st = style();
    let mut counts: BTreeMap<Level, usize> = BTreeMap::new();
    let mut fixable = 0;
    let mut dangerous = 0;
    for r in reports {
        let toolchain = rustc.map_or(String::new(), |v| format!(", rustc {v}"));
        println!(
            "{} {} {}",
            st.bold("cargo modernize:"),
            st.bold(format!("{} v{}", r.name, r.version)),
            st.dim(format!("(edition {}, rust-version {}{toolchain})", r.edition, r.rust_version))
        );
        if r.findings.is_empty() {
            println!("  {} nothing to modernize\n", st.green("✓"));
            continue;
        }
        for f in &r.findings {
            *counts.entry(f.level).or_default() += 1;
            fixable += usize::from(f.fix.as_ref().is_some_and(|x| !x.dangerous));
            dangerous += usize::from(f.fix.as_ref().is_some_and(|x| x.dangerous));
        }
        let (actionable, opinions): (Vec<&Finding>, Vec<&Finding>) = r.findings.iter().partition(|f| f.level >= Level::Warn);
        print_groups(&st, &actionable, "▸");
        if !opinions.is_empty() {
            println!(
                "\n{} {}",
                st.magenta("━━"),
                st.bold("Suggestions: newer ways to write this (opinions; adopt what you like)")
            );
            print_groups(&st, &opinions, "◦");
        }
        println!();
    }
    let total: usize = counts.values().sum();
    if total > 0 {
        let parts: Vec<String> = counts.iter().rev().map(|(l, n)| format!("{n} {}", st.level(*l))).collect();
        let mut hint = String::new();
        if fixable > 0 {
            hint += &format!("; {fixable} safe rewrite(s) with `cargo modernize --fix`");
        }
        if dangerous > 0 {
            hint += &format!("; {dangerous} more with `--dangerous`");
        }
        println!("{}{hint}", st.bold(format!("{total} finding(s): {}", parts.join(", "))));
        println!("{}", st.dim("`cargo modernize --explain <rule>` shows real-world examples; tune levels per folder in `.modernize`"));
    }
    println!("{}", st.dim(coverage_line(reports, unanalyzed)));
}

fn print_groups(st: &Style, findings: &[&Finding], bullet: &str) {
    let mut groups: BTreeMap<(u8, Category, String), Vec<&Finding>> = BTreeMap::new();
    for f in findings {
        groups.entry(group_key(f)).or_default().push(f);
    }
    for ((_, _, title), mut items) in groups {
        items.sort_by(|a, b| {
            let manifest = |f: &Finding| !f.file.ends_with("Cargo.toml");
            (manifest(a), &a.file, a.line, a.column).cmp(&(manifest(b), &b.file, b.line, b.column))
        });
        let nfix = items.iter().filter(|f| f.fix.is_some()).count();
        let fixnote = if nfix > 0 { format!(", {nfix} auto-fixable") } else { String::new() };
        println!("\n{} {}  {}", st.cyan(bullet), st.bold(&title), st.dim(format!("{} finding(s){fixnote}", items.len())));
        let mut docs: Vec<&str> = Vec::new();
        for f in items {
            println!(
                "  {}  {} {}",
                st.bold(format!("{}:{}:{}", f.file.display(), f.line, f.column)),
                st.level(f.level),
                st.dim(&f.rule.id)
            );
            println!("    {}", f.rule.message);
            if let Some(d) = &f.detail {
                println!("    {d}");
            }
            if !f.snippet.is_empty() {
                println!("    {} {}", st.dim("│"), f.snippet);
            }
            match (&f.fix, &f.rule.suggestion) {
                (Some(fix), _) => {
                    let e = &fix.edits[0];
                    let tag = if fix.dangerous { st.yellow(" (dangerous)") } else { String::new() };
                    println!("    {} {} → {}{tag}", st.green("= fix:"), one_line(&e.old), st.green(one_line(&e.new)));
                }
                (None, Some(s)) => println!("    {} {s}", st.cyan("= help:")),
                _ => {}
            }
            if let Some(n) = &f.note {
                println!("    {} {n}", st.yellow("= note:"));
            }
            if let Some(d) = f.rule.docs.as_deref().filter(|d| !docs.contains(d)) {
                docs.push(d);
            }
        }
        for d in docs {
            println!("  {} {d}", st.dim("see:"));
        }
    }
}

fn one_line(s: &str) -> String {
    let flat = s.split_whitespace().collect::<Vec<_>>().join(" ");
    if flat.chars().count() > 60 { format!("{}…", flat.chars().take(59).collect::<String>()) } else { flat }
}

fn level_word(l: Level) -> &'static str {
    match l {
        Level::Deny => "error",
        Level::Warn => "warning",
        Level::Suggest | Level::Off => "note",
    }
}

/// rustc-style diagnostics: `warning[rule]: message`, a source excerpt with carets,
/// notes, and the suggested rewrite as a `-`/`+` diff.
pub fn rustc(reports: &[PackageReport], unanalyzed: &[String]) {
    let st = style();
    let paint = |l: Level, s: &str| match l {
        Level::Deny => st.paint("1;31", s),
        Level::Warn => st.paint("1;33", s),
        _ => st.paint("1;32", s),
    };
    let mut texts: HashMap<std::path::PathBuf, String> = HashMap::new();
    for r in reports {
        let mut findings: Vec<&Finding> = r.findings.iter().collect();
        findings.sort_by(|a, b| (&a.file, a.line, a.column).cmp(&(&b.file, b.line, b.column)));
        for f in &findings {
            let width = f.end_line.max(f.line).to_string().len().max(
                f.fix.as_ref().and_then(|x| x.edits.first()).map_or(1, |e| {
                    let t = texts.entry(e.path.clone()).or_insert_with(|| std::fs::read_to_string(&e.path).unwrap_or_default());
                    let (first, old, new) = hunk(t, &[e]);
                    (first + old.len().max(new.len())).to_string().len()
                }),
            );
            let pad = " ".repeat(width);
            let bar = st.blue("|");
            println!("{}: {}", paint(f.level, &format!("{}[{}]", level_word(f.level), f.rule.id)), st.bold(&f.rule.message));
            println!("{pad}{} {}:{}:{}", st.blue("-->"), f.file.display(), f.line, f.column);
            println!("{pad} {bar}");
            println!("{} {bar} {}", st.blue(format!("{:>width$}", f.line)), f.source_line);
            let line_len = f.source_line.chars().count() + 1;
            let end = if f.end_line == f.line { f.end_column.max(f.column + 1) } else { line_len };
            let carets = "^".repeat(end.saturating_sub(f.column).max(1));
            let more = if f.end_line > f.line { st.dim(" …") } else { String::new() };
            println!("{pad} {bar} {}{}{more}", " ".repeat(f.column - 1), paint(f.level, &carets));
            println!("{pad} {bar}");
            if let Some(d) = &f.detail {
                println!("{pad} {} {}: {d}", st.blue("="), st.bold("note"));
            }
            if let Some(n) = &f.note {
                println!("{pad} {} {}: {n}", st.blue("="), st.bold("note"));
            }
            if f.fix.is_none() {
                if let Some(s) = &f.rule.suggestion {
                    println!("{pad} {} {}: {s}", st.blue("="), st.bold("help"));
                }
            }
            if let Some(d) = &f.rule.docs {
                println!("{pad} {} {}: for more information, see {d}", st.blue("="), st.bold("note"));
            }
            if let Some(fix) = &f.fix {
                let how = if fix.dangerous { "`cargo modernize --dangerous`" } else { "`cargo modernize --fix`" };
                let label = if fix.dangerous { "rewrite (dangerous: may change behavior or not compile)" } else { "rewrite" };
                println!("{}: {label}, applied by {how}", st.paint("1;36", "help"));
                println!("{pad} {bar}");
                for e in &fix.edits {
                    let t = texts.entry(e.path.clone()).or_insert_with(|| std::fs::read_to_string(&e.path).unwrap_or_default());
                    let (first, old, new) = hunk(t, &[e]);
                    for (i, l) in old.iter().enumerate() {
                        println!("{} {} {}", st.blue(format!("{:>width$}", first + i)), st.red("-"), st.red(l));
                    }
                    for (i, l) in new.iter().enumerate() {
                        println!("{} {} {}", st.blue(format!("{:>width$}", first + i)), st.green("+"), st.green(l));
                    }
                }
                println!("{pad} {bar}");
            }
            println!();
        }
        let count = |l: Level| r.findings.iter().filter(|f| f.level == l).count();
        let safe = r.findings.iter().filter(|f| f.fix.as_ref().is_some_and(|x| !x.dangerous)).count();
        let dangerous = r.findings.iter().filter(|f| f.fix.as_ref().is_some_and(|x| x.dangerous)).count();
        let parts: Vec<String> = [(Level::Deny, "error"), (Level::Warn, "warning"), (Level::Suggest, "note")]
            .iter()
            .filter(|(l, _)| count(*l) > 0)
            .map(|(l, w)| format!("{} {w}{}", count(*l), if count(*l) == 1 { "" } else { "s" }))
            .collect();
        if parts.is_empty() {
            println!("{} `{}` is modern", st.green("    Finished"), r.name);
            continue;
        }
        let mut hint = Vec::new();
        if safe > 0 {
            hint.push(format!("run `cargo modernize --fix` to apply {safe} safe rewrite{}", if safe == 1 { "" } else { "s" }));
        }
        if dangerous > 0 {
            hint.push(format!("`--dangerous` for {dangerous} more"));
        }
        let hint = if hint.is_empty() { String::new() } else { format!(" ({})", hint.join("; ")) };
        let worst = r.findings.iter().map(|f| f.level).max().unwrap_or(Level::Suggest);
        println!("{}: `{}` generated {}{hint}", paint(worst, level_word(worst)), r.name, parts.join(", "));
    }
    println!("{}", st.dim(coverage_line(reports, unanalyzed)));
}

pub fn json(reports: &[PackageReport], unanalyzed: &[String]) {
    let packages: Vec<_> = reports
        .iter()
        .map(|r| {
            let findings: Vec<_> = r
                .findings
                .iter()
                .map(|f| {
                    json!({
                        "rule": f.rule.id,
                        "level": f.level,
                        "category": f.rule.category,
                        "plan": f.rule.plan,
                        "message": f.rule.message,
                        "detail": f.detail,
                        "file": f.file,
                        "line": f.line,
                        "column": f.column,
                        "snippet": f.snippet,
                        "suggestion": f.rule.suggestion,
                        "note": f.note,
                        "docs": f.rule.docs,
                        "end_line": f.end_line,
                        "end_column": f.end_column,
                        "fix": f.fix.as_ref().map(|fix| json!({
                            "dangerous": fix.dangerous,
                            "edits": fix.edits.iter().map(|e| json!({ "file": e.path, "old": e.old, "new": e.new })).collect::<Vec<_>>(),
                        })),
                    })
                })
                .collect();
            json!({
                "name": r.name,
                "version": r.version,
                "edition": r.edition,
                "rust_version": r.rust_version,
                "coverage": coverage_json(&Totals::of(std::slice::from_ref(r)), &r.coverage.skipped),
                "findings": findings,
            })
        })
        .collect();
    let all_skipped: Vec<_> = reports.iter().flat_map(|r| r.coverage.skipped.iter().cloned()).collect();
    let mut coverage = coverage_json(&Totals::of(reports), &all_skipped);
    coverage["unanalyzed_crates"] = json!(unanalyzed);
    println!("{}", serde_json::to_string_pretty(&json!({ "packages": packages, "coverage": coverage })).unwrap());
}

fn coverage_json(t: &Totals, skipped: &[crate::scan::SkippedFile]) -> serde_json::Value {
    json!({
        "files_scanned": t.files,
        "lines_scanned": t.lines,
        "files_skipped": t.skipped_files,
        "lines_skipped": t.skipped_lines,
        "skipped_pct": (t.skipped_pct() * 10.0).round() / 10.0,
        "skipped": skipped.iter().map(|f| json!({ "file": f.file, "lines": f.lines, "error": f.error })).collect::<Vec<_>>(),
    })
}

pub fn explain(r: &Rule) {
    let st = style();
    println!("{} {}  {}", st.bold(&r.id), st.level(r.default_level()), st.dim(r.category.title()));
    if let Some(p) = &r.plan {
        println!("{} {p}", st.dim("plan:"));
    }
    println!("\n{}\n", r.message);
    let gates = [
        r.krate.as_ref().map(|c| format!("crate {c} {}", r.version.as_deref().unwrap_or("*"))),
        r.since_rust.as_ref().map(|v| format!("Rust ≥ {v}")),
        r.min_edition.map(|e| format!("edition ≥ {e}")),
        r.below_edition.map(|e| format!("edition < {e}")),
    ];
    for g in gates.into_iter().flatten() {
        println!("  {} {g}", st.dim("applies to:"));
    }
    if let Some(s) = &r.suggestion {
        println!("  {} {s}", st.cyan("help:"));
    }
    for (i, ex) in r.examples.iter().enumerate() {
        let src = ex.source.as_deref().map_or(String::new(), |s| format!("  {}", st.dim(format!("({s})"))));
        println!("\n{}{src}", st.bold(format!("Example {}", i + 1)));
        for l in ex.before.trim_end().lines() {
            println!("  {} {l}", st.red("-"));
        }
        for l in ex.after.trim_end().lines() {
            println!("  {} {l}", st.green("+"));
        }
    }
    if let Some(d) = &r.docs {
        println!("\n{} {d}", st.dim("see:"));
    }
    println!(
        "\n{}",
        st.dim(format!("Disable for a folder with a `.modernize` file:  [rules] \"{}\" = \"off\"", r.id))
    );
}

pub fn list_rules(rules: &[Rule]) {
    let st = style();
    let mut sorted: Vec<&Rule> = rules.iter().collect();
    sorted.sort_by(|a, b| (a.category, &a.plan, &a.id).cmp(&(b.category, &b.plan, &b.id)));
    let mut last = None;
    for r in sorted {
        if last != Some(r.category) {
            println!("\n{}", st.bold(r.category.title()));
            last = Some(r.category);
        }
        let gate = [
            r.krate.as_ref().map(|c| format!("{c} {}", r.version.as_deref().unwrap_or("*"))),
            r.since_rust.as_ref().map(|v| format!("rust ≥ {v}")),
        ]
        .into_iter()
        .flatten()
        .collect::<Vec<_>>()
        .join(", ");
        let fix = if r.rename.is_some() || r.to_version.is_some() { st.green(" [fix]") } else { String::new() };
        let ex = if r.examples.is_empty() { String::new() } else { st.dim(format!(" [{} example(s)]", r.examples.len())) };
        println!("  {:<34} {:<8} {}{fix}{ex}", st.cyan(&r.id), st.level(r.default_level()), st.dim(gate));
    }
}
