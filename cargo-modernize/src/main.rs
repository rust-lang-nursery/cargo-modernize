mod config;
mod ecosystem;
mod fix;
mod project;
mod report;
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

/// Find code that the ecosystem has moved past: crate upgrade plans
/// (e.g. rand → 0.10), crates now covered by std (lazy_static, num_cpus, …),
/// unmaintained dependencies, edition migrations, and (as opinions) newer
/// ways to be Rusty. Configure per folder with `.modernize` files.
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
    /// Apply automatic fixes (identifier renames, version bumps). Commit first.
    #[arg(long)]
    fix: bool,
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
    Human,
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

    let mut ws = project::load(args.manifest_path.as_deref())?;
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

    let mut reports = analyze(&ws)?;
    if args.fix {
        let n = fix::apply(&reports)?;
        eprintln!("applied {n} fix(es); re-checking…\n");
        ws = project::load(args.manifest_path.as_deref())?; // versions may have been bumped
        reports = analyze(&ws)?;
    }
    match args.format {
        Format::Human => {
            report::human(&reports, rustc.as_ref());
            if reports.iter().flat_map(|r| &r.findings).any(|f| f.rule.kind == rules::Kind::Ecosystem) {
                println!("ecosystem data from {}; refresh with `cargo run -p modernize-ecosystem` or pass --ecosystem FILE", eco.generated);
            }
        }
        Format::Json => report::json(&reports),
    }
    let failed = reports.iter().flat_map(|r| &r.findings).any(|f| args.fail_on != Level::Off && f.level >= args.fail_on);
    Ok(if failed { ExitCode::from(2) } else { ExitCode::SUCCESS })
}
