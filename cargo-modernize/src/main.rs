mod config;
mod ecosystem;
mod fix;
mod legacy;
mod project;
mod report;
mod rewrite;
mod rules;
mod scan;

use anyhow::Result;
use clap::{Parser, ValueEnum};
use rules::{Category, Level};
use std::path::PathBuf;
use std::process::ExitCode;

#[derive(Parser)]
#[command(name = "cargo", bin_name = "cargo")]
enum Cargo {
    Modernize(Args),
}

/// Find code that the ecosystem has moved past, report it like rustc, and rewrite it
/// like rustfmt: crate upgrade plans (e.g. rand → 0.10), crates now covered by std
/// (lazy_static, num_cpus, …), unmaintained dependencies, edition migrations, and
/// newer ways to be Rusty. Configure per folder with `.modernize` files.
///
/// `--fix` applies behavior-preserving rewrites. `--dangerous` modernizes everything
/// it can (version bumps, std replacements, API migrations); it then runs `cargo check`
/// and reverts files that no longer compile.
#[derive(clap::Args)]
#[command(version)]
struct Args {
    /// Path to Cargo.toml
    #[arg(long, value_name = "PATH")]
    manifest_path: Option<PathBuf>,
    /// Only analyze these workspace packages
    #[arg(short, long, value_name = "SPEC")]
    package: Vec<String>,
    #[arg(long, value_enum, default_value_t = Format::Human)]
    format: Format,
    /// Rewrite files with the safe (behavior-preserving) fixes
    #[arg(long)]
    fix: bool,
    /// Rewrite everything that can be modernized, including fixes that may change
    /// behavior or break the build (implies --fix)
    #[arg(long)]
    dangerous: bool,
    /// Show the rewrites as a diff instead of writing them (exit 1 if there are any).
    /// Previews one pass; a real run repeats until nothing more applies
    #[arg(long)]
    diff: bool,
    /// Rewrite even with uncommitted changes
    #[arg(long)]
    allow_dirty: bool,
    /// Rewrite outside a git repository
    #[arg(long)]
    allow_no_vcs: bool,
    /// Don't `cargo check` after rewriting (and don't revert broken files)
    #[arg(long)]
    no_verify: bool,
    /// Don't run rustfmt on rewritten files
    #[arg(long)]
    no_format: bool,
    /// Ecosystem data file from `modernize-ecosystem` (default: the built-in snapshot)
    #[arg(long, value_name = "FILE", env = "CARGO_MODERNIZE_ECOSYSTEM")]
    ecosystem: Option<PathBuf>,
    /// Extra rule pack files ([[rule]] tables)
    #[arg(long = "rules", value_name = "FILE")]
    extra_rules: Vec<PathBuf>,
    /// Don't load the built-in rule packs
    #[arg(long)]
    no_builtin_rules: bool,
    /// Only report these categories
    #[arg(long, value_enum)]
    category: Vec<Category>,
    /// Only report plans/rules whose name or id contains TEXT (e.g. "rand")
    #[arg(long, value_name = "TEXT")]
    only: Option<String>,
    /// Hide findings below this level (`warn` hides opinions)
    #[arg(long, value_enum, default_value_t = Level::Suggest, value_name = "LEVEL")]
    min_level: Level,
    /// Exit with status 2 if any finding is at or above this level (CI: `--fail-on warn`)
    #[arg(long, value_enum, default_value_t = Level::Deny, value_name = "LEVEL")]
    fail_on: Level,
    /// Print the rule database and exit
    #[arg(long)]
    list_rules: bool,
    /// Explain a rule, with real-world before/after examples
    #[arg(long, value_name = "RULE")]
    explain: Option<String>,
}

#[derive(Clone, Copy, ValueEnum)]
enum Format {
    /// rustc-style diagnostics
    Human,
    /// Grouped by upgrade plan and category
    Plans,
    Json,
}

fn main() -> ExitCode {
    let Cargo::Modernize(args) = Cargo::parse();
    match run(args) {
        Ok(code) => code,
        Err(e) => {
            eprintln!("error: {e:#}");
            ExitCode::from(101)
        }
    }
}

fn run(args: Args) -> Result<ExitCode> {
    let mut rules = if args.no_builtin_rules { Vec::new() } else { rules::builtin()? };
    for path in &args.extra_rules {
        rules::merge(&mut rules, rules::load_file(path)?);
    }
    if let Some(id) = &args.explain {
        return Ok(match rules.iter().find(|r| r.id == *id) {
            Some(r) => {
                report::explain(r);
                ExitCode::SUCCESS
            }
            None => {
                eprintln!("error: no rule `{id}` (see --list-rules)");
                ExitCode::from(101)
            }
        });
    }
    if args.list_rules && args.manifest_path.is_none() {
        report::list_rules(&rules);
        return Ok(ExitCode::SUCCESS);
    }

    let ws = project::load(args.manifest_path.as_deref())?;
    let skipped = scan::unanalyzed_crates(&ws.packages);
    if !skipped.is_empty() {
        let shown: Vec<_> = skipped.iter().take(5).map(|p| p.strip_prefix(&ws.root).unwrap_or(p).display().to_string()).collect();
        eprintln!(
            "warning: {} nested crate(s) are not workspace members and were not analyzed: {}{}\n  add them to `[workspace] members`, or run `cargo modernize --manifest-path <crate>/Cargo.toml` on each",
            skipped.len(),
            shown.join(", "),
            if skipped.len() > shown.len() { ", …" } else { "" }
        );
    }
    let (config, scoped) = config::Config::discover(&ws.root)?;
    rules.extend(scoped);
    if args.list_rules {
        report::list_rules(&rules);
        return Ok(ExitCode::SUCCESS);
    }
    rules.retain(|r| args.category.is_empty() || args.category.contains(&r.category));
    if let Some(text) = &args.only {
        rules.retain(|r| r.id.contains(text.as_str()) || r.plan.as_deref().is_some_and(|p| p.contains(text.as_str())));
    }

    let rustc = project::rustc_version();
    let eco = ecosystem::load(args.ecosystem.as_deref())?;
    let analyze = |ws: &project::Workspace| -> Result<Vec<scan::PackageReport>> {
        let cx = scan::Context { ws_root: &ws.root, rules: &rules, config: &config, rustc: rustc.as_ref(), ecosystem: &eco, lock: &ws.lock };
        let mut reports = ws
            .packages
            .iter()
            .filter(|p| args.package.is_empty() || args.package.contains(&p.name))
            .map(|p| scan::analyze(p, &cx))
            .collect::<Result<Vec<_>>>()?;
        for r in &mut reports {
            r.findings.retain(|f| f.level >= args.min_level);
        }
        Ok(reports)
    };

    // Versions may be bumped by fixes, so each pass reloads the workspace.
    let fresh = || -> Result<Vec<scan::PackageReport>> { analyze(&project::load(args.manifest_path.as_deref())?) };

    if args.fix || args.dangerous || args.diff {
        let opts = fix::Options {
            dangerous: args.dangerous,
            diff: args.diff,
            verify: !args.no_verify,
            format: !args.no_format,
            allow_dirty: args.allow_dirty,
            allow_no_vcs: args.allow_no_vcs,
            manifest_path: args.manifest_path.clone(),
            ws_root: ws.root.clone(),
            editions: ws.packages.iter().map(|p| (p.root.clone(), p.edition)).collect(),
        };
        let outcome = fix::run(&opts, fresh)?;
        if args.diff {
            return Ok(if outcome.applied > 0 { ExitCode::from(1) } else { ExitCode::SUCCESS });
        }
        summarize(&outcome, &ws.root);
    }

    let reports = fresh()?;
    match args.format {
        Format::Plans => report::plans(&reports, rustc.as_ref()),
        Format::Human => {
            report::rustc(&reports);
            if reports.iter().flat_map(|r| &r.findings).any(|f| f.rule.kind == rules::Kind::Ecosystem) {
                println!("ecosystem data from {}; refresh with `cargo run -p modernize-ecosystem` or pass --ecosystem FILE", eco.generated);
            }
        }
        Format::Json => report::json(&reports),
    }
    let failed = reports.iter().flat_map(|r| &r.findings).any(|f| args.fail_on != Level::Off && f.level >= args.fail_on);
    Ok(if failed { ExitCode::from(2) } else { ExitCode::SUCCESS })
}

fn summarize(o: &fix::Outcome, root: &std::path::Path) {
    let rel = |p: &std::path::Path| p.strip_prefix(root).unwrap_or(p).display().to_string();
    eprintln!(
        "  Modernized {} edit(s) across {} file(s){}",
        o.applied,
        o.changed.len() + o.reverted.len(),
        if o.formatted > 0 { format!("; rustfmt'd {}", o.formatted) } else { String::new() }
    );
    if !o.reverted.is_empty() {
        eprintln!("  Reverted {} file(s) whose rewrites didn't compile: {}", o.reverted.len(), o.reverted.iter().map(|p| rel(p)).collect::<Vec<_>>().join(", "));
    }
    match o.verified {
        Some(true) => eprintln!("  Verified: `cargo check` passes"),
        Some(false) => eprintln!("  warning: `cargo check` still fails after reverting"),
        None => {}
    }
    eprintln!();
}
