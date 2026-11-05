//! Generates `cargo-modernize/rules/ecosystem.toml`: the latest version of popular and
//! at-risk crates, their maintenance status with evidence, and the successors worth
//! considering. Run it periodically (e.g. weekly in CI); cargo-modernize reads the
//! result offline.
//!
//! Signals, strongest first:
//! - RustSec `unmaintained` advisories and unpatched vulnerabilities
//! - the maintainer's own marker: a `+deprecated` release or a description saying so
//! - every version yanked; GitHub repository archived or gone
//! - at-risk: no release in 3 years *and* no pushes in 2 years *and* unmet demand
//!   (open "is this maintained?" issues or a pile of open issues). Quiet alone isn't
//!   abandoned: plenty of finished crates never need a release.
//!
//! Successors are scored on evidence: named by an advisory, a GitHub fork of the
//! original that publishes a crate, a description that says "maintained fork of …",
//! recent releases, and share of recent downloads.

mod http;
mod rustsec;
mod sources;

use anyhow::{Context, Result};
use clap::Parser;
use rustsec::{Advisory, Alternative};
use serde::Serialize;
use sources::{CrateInfo, GitHub, Repo, RepoLookup};
use std::collections::{BTreeMap, BTreeSet};
use std::path::PathBuf;
use std::time::Duration;

#[derive(Parser)]
#[command(version, about)]
struct Args {
    /// Output file
    #[arg(long, default_value = "cargo-modernize/rules/ecosystem.toml")]
    out: PathBuf,
    /// Include the N most-downloaded crates (for latest-version data)
    #[arg(long, default_value_t = 300)]
    top: usize,
    /// Extra crates to include
    #[arg(long, value_delimiter = ',')]
    crates: Vec<String>,
    /// Include every registry package from these Cargo.lock files
    #[arg(long, value_name = "Cargo.lock")]
    lock: Vec<PathBuf>,
    #[arg(long, default_value = "target/ecosystem-cache")]
    cache_dir: PathBuf,
    /// Re-fetch cached responses older than this
    #[arg(long, default_value_t = 24)]
    ttl_hours: u64,
    /// Contact info for the User-Agent (crates.io's crawler policy asks for one)
    #[arg(long, env = "MODERNIZE_CONTACT")]
    contact: Option<String>,
    /// Skip GitHub (archived flags, forks, issue signals)
    #[arg(long)]
    no_github: bool,
    /// Only analyze --crates/--lock (skip the advisory and top-N sets); for testing
    #[arg(long)]
    only_listed: bool,
}

#[derive(Serialize, Default)]
struct Entry {
    #[serde(skip_serializing_if = "Option::is_none")]
    latest: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    last_release: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    recent_downloads: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    status: Option<&'static str>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    evidence: Vec<String>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    advisories: Vec<String>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    std_successors: Vec<String>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    successors: Vec<Successor>,
}

#[derive(Serialize, Clone)]
struct Successor {
    name: String,
    score: u32,
    recommended: bool,
    why: Vec<String>,
}

#[derive(Serialize)]
struct Output {
    generated: String,
    sources: Vec<String>,
    crates: BTreeMap<String, Entry>,
}

const DAY: i64 = 86_400;

fn main() -> Result<()> {
    let args = Args::parse();
    let ua = match &args.contact {
        Some(c) => format!("modernize-ecosystem/{} ({c})", env!("CARGO_PKG_VERSION")),
        None => {
            eprintln!("note: pass --contact (or MODERNIZE_CONTACT) so crates.io can reach you, per its crawler policy");
            format!("modernize-ecosystem/{}", env!("CARGO_PKG_VERSION"))
        }
    };
    let token = (!args.no_github).then(github_token).flatten();
    if !args.no_github && token.is_none() {
        eprintln!("note: no GITHUB_TOKEN or `gh auth token`; GitHub lookups are limited to 60/hour");
    }
    let http = http::Http::new(&ua, args.cache_dir.clone(), Duration::from_secs(args.ttl_hours * 3600), token)?;
    let gh = GitHub::new(&http, !args.no_github);
    let now = jiff::Timestamp::now().as_second();

    eprintln!("loading RustSec advisory-db…");
    let advisories = rustsec::load(&http)?;
    eprintln!("  {} crates with advisories", advisories.len());

    // Candidate set: anything RustSec calls unmaintained or leaves unpatched, plus popular crates.
    let mut names: BTreeSet<String> = BTreeSet::new();
    if !args.only_listed {
        names.extend(
            advisories
                .iter()
                .filter(|(_, a)| a.iter().any(|a| a.is_unmaintained() || a.is_unpatched_vulnerability()))
                .map(|(n, _)| n.clone()),
        );
        eprintln!("fetching top {} crates…", args.top);
        names.extend(sources::top_crates(&http, args.top)?);
    }
    names.extend(args.crates.iter().cloned());
    for lock in &args.lock {
        names.extend(lock_packages(lock)?);
    }

    let mut out = BTreeMap::new();
    let total = names.len();
    for (i, name) in names.iter().enumerate() {
        if i % 25 == 0 {
            eprintln!("[{i}/{total}] {name}");
        }
        let Some(info) = sources::crate_info(&http, name)? else { continue };
        let advs = advisories.get(name).map(Vec::as_slice).unwrap_or(&[]);
        out.insert(info.name.clone(), analyze(&http, &gh, &info, advs, &advisories, now)?);
    }

    let flagged = out.values().filter(|e| e.status.is_some()).count();
    let doc = Output {
        generated: jiff::Timestamp::now().strftime("%Y-%m-%d").to_string(),
        sources: vec!["rustsec/advisory-db@main".into(), "crates.io".into(), "github.com".into()],
        crates: out,
    };
    let header = "# Generated by modernize-ecosystem; do not edit. Regenerate with:\n#   cargo run -p modernize-ecosystem -- --contact <you>\n";
    std::fs::write(&args.out, format!("{header}{}", toml::to_string(&doc)?)).with_context(|| format!("writing {}", args.out.display()))?;
    let stats = http.stats.lock().unwrap();
    eprintln!(
        "wrote {} ({} crates, {flagged} flagged; {} fetched, {} from cache)",
        args.out.display(),
        doc.crates.len(),
        stats.fetched,
        stats.cached
    );
    Ok(())
}

fn analyze(
    http: &http::Http,
    gh: &GitHub,
    info: &CrateInfo,
    advs: &[Advisory],
    all_advisories: &BTreeMap<String, Vec<Advisory>>,
    now: i64,
) -> Result<Entry> {
    let mut e = Entry { latest: info.latest.clone(), last_release: info.last_release.clone(), ..Entry::default() };
    let mut statuses: Vec<&'static str> = Vec::new();

    let label = |a: &Advisory| if a.id == "RUSTSEC-0000-0000" { "RustSec advisory (ID pending)".to_owned() } else { a.id.clone() };
    for a in advs {
        if a.is_unmaintained() {
            statuses.push("unmaintained");
            e.evidence.push(format!("{} ({}): {}", label(a), a.date, a.title));
            e.advisories.push(a.id.clone());
        } else if a.is_unpatched_vulnerability() {
            statuses.push("vulnerable");
            e.evidence.push(format!("{} ({}): {} (no patched version)", label(a), a.date, a.title));
            e.advisories.push(a.id.clone());
        }
    }
    if let Some(v) = &info.deprecated_version {
        statuses.push("deprecated");
        e.evidence.push(format!("latest release is `{v}`"));
    }
    let lower = info.description.to_ascii_lowercase();
    if ["deprecated", "unmaintained", "no longer maintained", "not maintained", "superseded", "abandoned"].iter().any(|p| lower.contains(p)) {
        statuses.push("deprecated");
        e.evidence.push(format!("crates.io description: \"{}\"", info.description));
    }
    if info.all_yanked {
        statuses.push("yanked");
        e.evidence.push("every published version is yanked".into());
    }

    let age_days = |date: &str| -> Option<i64> {
        let ts: jiff::Timestamp = if date.len() == 10 { format!("{date}T00:00:00Z").parse().ok()? } else { date.parse().ok()? };
        Some((now - ts.as_second()) / DAY)
    };
    let release_age = info.last_release.as_deref().and_then(age_days);

    // GitHub: archived, gone, moved, and (for quiet crates) unmet demand.
    let full = info.repository.as_deref().and_then(sources::github_full_name);
    let mut repo: Option<Repo> = None;
    if let Some(full) = &full {
        match gh.repo(full) {
            RepoLookup::Found(r) => {
                if r.archived {
                    statuses.push("archived");
                    e.evidence.push(format!("GitHub repository {} is archived", r.full_name));
                }
                if !r.full_name.eq_ignore_ascii_case(full) {
                    e.evidence.push(format!("repository moved: {full} → {}", r.full_name));
                }
                repo = Some(r);
            }
            RepoLookup::Missing => e.evidence.push(format!("repository github.com/{full} no longer exists")),
            RepoLookup::Unknown => {}
        }
    }
    let quiet = release_age.is_some_and(|d| d > 3 * 365);
    if statuses.is_empty() && quiet {
        let push_age = repo.as_ref().and_then(|r| r.pushed_at.as_deref()).and_then(age_days);
        let gone = repo.is_none() && e.evidence.iter().any(|x| x.contains("no longer exists"));
        if gone || push_age.is_some_and(|d| d > 2 * 365) {
            let issues = repo.as_ref().map(|r| gh.maintenance_issues(&r.full_name)).unwrap_or_default();
            let open = repo.as_ref().map_or(0, |r| r.open_issues_count);
            if gone || !issues.is_empty() || open >= 15 {
                statuses.push("at-risk");
                e.evidence.push(format!("no release in {} years", release_age.unwrap_or(0) / 365));
                if let Some(d) = push_age {
                    e.evidence.push(format!("no commits in {} years", d / 365));
                }
                if let Some(i) = issues.first() {
                    e.evidence.push(format!("open issue #{}: \"{}\"", i.number, i.title));
                }
                if open >= 15 {
                    e.evidence.push(format!("{open} open issues/PRs"));
                }
            }
        }
    }

    // Primary status, most severe first.
    for s in ["yanked", "deprecated", "unmaintained", "archived", "vulnerable", "at-risk"] {
        if statuses.contains(&s) {
            e.status = Some(s);
            break;
        }
    }
    let needs_successor = e.status.is_some_and(|s| s != "vulnerable");
    if needs_successor {
        e.recent_downloads = Some(info.recent_downloads);
        for a in advs {
            for alt in &a.alternatives {
                if let Alternative::Std(s) = alt {
                    if !e.std_successors.contains(s) {
                        e.std_successors.push(s.clone());
                    }
                }
            }
        }
        e.successors = successors(http, gh, info, repo.as_ref(), advs, all_advisories, &age_days)?;
    }
    Ok(e)
}

fn successors(
    http: &http::Http,
    gh: &GitHub,
    orig: &CrateInfo,
    orig_repo: Option<&Repo>,
    advs: &[Advisory],
    all_advisories: &BTreeMap<String, Vec<Advisory>>,
    age_days: &dyn Fn(&str) -> Option<i64>,
) -> Result<Vec<Successor>> {
    let mut why: BTreeMap<String, (u32, Vec<String>)> = BTreeMap::new();
    fn add(why: &mut BTreeMap<String, (u32, Vec<String>)>, name: &str, pts: u32, reason: String) {
        let entry = why.entry(name.to_owned()).or_default();
        entry.0 += pts;
        entry.1.push(reason);
    }

    for a in advs {
        for alt in &a.alternatives {
            match alt {
                Alternative::Crate(c) => add(&mut why, c, 3, format!("named as an alternative in {}", a.id)),
                Alternative::Mentioned(c) => add(&mut why, c, 2, format!("recommended in the text of {}", a.id)),
                Alternative::Std(_) => {}
            }
        }
    }
    // The maintainer's own pointer: "Consider tar-codec instead", "renamed to `cc`", "in favor of `smol`".
    for name in description_successors(&orig.description) {
        if name != orig.name {
            add(&mut why, &name, 3, "the crate's own description recommends it".into());
        }
    }
    // Forks of the original repository that publish a crate.
    if let Some(r) = orig_repo {
        for fork in gh.forks(&r.full_name).into_iter().filter(|f| f.stargazers_count >= 2).take(8) {
            if let Some(name) = gh.published_name(&fork, &orig.name) {
                if name != orig.name && sources::exists(http, &name)? {
                    add(&mut why, &name, 2, format!("published from {}, a GitHub fork of {}", fork.full_name, r.full_name));
                }
            }
        }
    }
    // Conventional successor names: foo2, foo-ng, dotenvy, pastey, …
    let n = &orig.name;
    let sep = if n.contains('_') { "_" } else { "-" };
    let variants = [
        format!("{n}2"),
        format!("{n}{sep}ng"),
        format!("{n}{sep}next"),
        format!("{n}{sep}fork"),
        format!("{n}{sep}maintained"),
        format!("{n}{sep}community"),
        format!("{n}y"),
    ];
    for v in variants {
        if !why.contains_key(&v) && sources::exists(http, &v)? {
            add(&mut why, &v, 0, "conventional successor name".into());
        }
    }

    let orig_age = orig.last_release.as_deref().and_then(age_days);
    let mut out = Vec::new();
    for (name, (mut score, mut reasons)) in why {
        let Some(info) = sources::crate_info(http, &name)? else { continue };
        if all_advisories.get(&info.name).is_some_and(|a| a.iter().any(Advisory::is_unmaintained)) {
            continue; // don't recommend one unmaintained crate for another
        }
        // Repository and description evidence.
        if let (Some(full), Some(orig_repo)) = (info.repository.as_deref().and_then(sources::github_full_name), orig_repo) {
            if !reasons.iter().any(|r| r.contains("GitHub fork")) {
                if let RepoLookup::Found(repo) = gh.repo(&full) {
                    if repo.parent.as_ref().is_some_and(|p| p.full_name.eq_ignore_ascii_case(&orig_repo.full_name)) {
                        score += 2;
                        reasons.push(format!("{full} is a GitHub fork of {}", orig_repo.full_name));
                    }
                }
            }
        }
        let desc = info.description.to_ascii_lowercase();
        let mentions = desc.contains(&orig.name.to_ascii_lowercase());
        if mentions && ["fork", "maintained", "continuation", "successor", "replacement", "drop-in"].iter().any(|w| desc.contains(w)) {
            score += 2;
            reasons.push(format!("describes itself as: \"{}\"", info.description));
        } else if mentions {
            score += 1;
            reasons.push(format!("description mentions {}", orig.name));
        } else if reasons.iter().any(|r| r == "conventional successor name") && ["fork", "drop-in", "maintained", "continuation"].iter().any(|w| desc.contains(w)) {
            score += 1;
            reasons.push(format!("describes itself as: \"{}\"", info.description));
        }
        let age = info.last_release.as_deref().and_then(age_days);
        if age.is_some_and(|a| a < 540 && orig_age.is_none_or(|o| a < o)) {
            score += 1;
            reasons.push(format!("released {}", info.last_release.clone().unwrap_or_default()));
        }
        let share = info.recent_downloads as f64 / (info.recent_downloads + orig.recent_downloads).max(1) as f64;
        if share >= 0.2 {
            score += 1;
            reasons.push(format!("{:.0}% of recent downloads between the two", share * 100.0));
        }
        if score >= 3 {
            out.push(Successor { name: info.name, score, recommended: false, why: reasons });
        }
    }
    out.sort_by(|a, b| b.score.cmp(&a.score).then(a.name.cmp(&b.name)));
    out.truncate(4);
    // Recommend only with an endorsement (the advisory names it) or strong combined evidence.
    let endorsed = |s: &Successor| {
        s.why.iter().any(|w| w.starts_with("named as an alternative") || w.starts_with("recommended in the text") || w.starts_with("the crate's own description"))
    };
    if let Some(top) = out.first_mut().filter(|s| endorsed(s) || s.score >= 5) {
        top.recommended = true;
    }
    Ok(out)
}

/// Crate names a description points to: "use X instead", "renamed to X", "in favor of X", …
fn description_successors(desc: &str) -> Vec<String> {
    const CUES: [&str; 8] = ["consider ", "use ", "renamed to ", "in favor of ", "in favour of ", "replaced by ", "superseded by ", "moved to "];
    let lower = desc.to_ascii_lowercase();
    let mut out = Vec::new();
    for cue in CUES {
        let mut from = 0;
        while let Some(i) = lower[from..].find(cue) {
            let start = from + i + cue.len();
            from = start;
            let after = &lower[start..];
            let after = after.strip_prefix("using ").unwrap_or(after);
            let start = start + (lower.len() - start - after.len());
            let word: String = lower[start..]
                .trim_start_matches(['`', '"', '\''])
                .chars()
                .take_while(|c| c.is_ascii_alphanumeric() || *c == '-' || *c == '_')
                .collect();
            let rest = &lower[start + word.len()..];
            // "use X instead" needs "instead"; the other cues stand alone.
            let needs_instead = matches!(cue, "consider " | "use ");
            if word.len() > 1 && !["the", "a", "an", "this", "it", "std", "crate"].contains(&word.as_str()) && (!needs_instead || rest.trim_start_matches(['`', '"', '\'']).trim_start().starts_with("instead")) && !out.contains(&word) {
                out.push(word);
            }
        }
    }
    out
}

fn github_token() -> Option<String> {
    std::env::var("GITHUB_TOKEN").ok().filter(|t| !t.is_empty()).or_else(|| {
        let out = std::process::Command::new("gh").args(["auth", "token"]).output().ok()?;
        out.status.success().then(|| String::from_utf8_lossy(&out.stdout).trim().to_owned()).filter(|t| !t.is_empty())
    })
}

fn lock_packages(path: &PathBuf) -> Result<Vec<String>> {
    let text = std::fs::read_to_string(path).with_context(|| format!("reading {}", path.display()))?;
    let lock: toml::Table = text.parse()?;
    Ok(lock
        .get("package")
        .and_then(|p| p.as_array())
        .into_iter()
        .flatten()
        .filter(|p| p.get("source").and_then(|s| s.as_str()).is_some_and(|s| s.starts_with("registry+")))
        .filter_map(|p| p.get("name")?.as_str().map(str::to_owned))
        .collect())
}

#[cfg(test)]
mod tests {
    use super::description_successors;

    #[test]
    fn successors_from_descriptions() {
        assert_eq!(description_successors("DEPRECATED. Consider tar-codec instead."), ["tar-codec"]);
        assert_eq!(description_successors("**Deprecated** crate, renamed to `cc` A build-time dependency"), ["cc"]);
        assert_eq!(description_successors("Deprecated in favor of `smol` - Async version of std"), ["smol"]);
        assert_eq!(description_successors("Unmaintained, consider using web-time instead"), ["web-time"]);
        assert_eq!(description_successors("Easy to use hashing for the web"), Vec::<String>::new());
    }
}
