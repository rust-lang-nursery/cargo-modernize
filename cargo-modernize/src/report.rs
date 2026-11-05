use crate::rules::{Category, Level, Rule};
use crate::scan::{Finding, PackageReport};
use serde_json::json;
use std::collections::BTreeMap;
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

pub fn human(reports: &[PackageReport], rustc: Option<&semver::Version>) {
    let st = style();
    let mut counts: BTreeMap<Level, usize> = BTreeMap::new();
    let mut fixable = 0;
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
            fixable += usize::from(f.fix.is_some());
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
        let hint = if fixable > 0 { format!("; {fixable} auto-fixable with `cargo modernize --fix`") } else { String::new() };
        println!("{}{hint}", st.bold(format!("{total} finding(s): {}", parts.join(", "))));
        println!("{}", st.dim("`cargo modernize --explain <rule>` shows real-world examples; tune levels per folder in `.modernize`"));
    }
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
                (Some(e), _) => println!("    {} {} → {}", st.green("= fix:"), e.old, st.green(&e.new)),
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

pub fn json(reports: &[PackageReport]) {
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
                        "fix": f.fix.as_ref().map(|e| json!({ "old": e.old, "new": e.new })),
                    })
                })
                .collect();
            json!({
                "name": r.name,
                "version": r.version,
                "edition": r.edition,
                "rust_version": r.rust_version,
                "findings": findings,
            })
        })
        .collect();
    println!("{}", serde_json::to_string_pretty(&json!({ "packages": packages })).unwrap());
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
