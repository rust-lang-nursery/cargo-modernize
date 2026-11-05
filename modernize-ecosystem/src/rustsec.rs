//! RustSec advisory-db: unmaintained notices, unpatched vulnerabilities, and the
//! alternatives advisories recommend.

use crate::http::{Fetched, Http};
use anyhow::{Context, Result, bail};
use serde::Deserialize;
use std::collections::BTreeMap;
use std::io::Read;

const TARBALL: &str = "https://github.com/rustsec/advisory-db/archive/refs/heads/main.tar.gz";

#[derive(Debug, Clone)]
pub struct Advisory {
    pub id: String,
    pub package: String,
    pub date: String,
    /// `unmaintained`, `unsound`, `notice`; `None` for a vulnerability.
    pub informational: Option<String>,
    pub patched: Vec<String>,
    pub title: String,
    pub alternatives: Vec<Alternative>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Alternative {
    /// Listed under an "Alternative(s)" heading.
    Crate(String),
    /// Only linked from the advisory's text ("recommends using the maintained [`web-time`]").
    Mentioned(String),
    Std(String),
}

impl Advisory {
    pub fn is_unmaintained(&self) -> bool {
        self.informational.as_deref() == Some("unmaintained")
    }

    pub fn is_unpatched_vulnerability(&self) -> bool {
        self.informational.is_none() && self.patched.is_empty()
    }
}

#[derive(Deserialize)]
struct Front {
    advisory: FrontAdvisory,
    #[serde(default)]
    versions: FrontVersions,
}

#[derive(Deserialize)]
struct FrontAdvisory {
    id: String,
    package: String,
    date: String,
    #[serde(default)]
    informational: Option<String>,
    #[serde(default)]
    withdrawn: Option<toml::Value>,
}

#[derive(Deserialize, Default)]
struct FrontVersions {
    #[serde(default)]
    patched: Vec<String>,
}

/// All live (not withdrawn) advisories for crates, keyed by package name.
pub fn load(http: &Http) -> Result<BTreeMap<String, Vec<Advisory>>> {
    let Fetched::Ok(bytes) = http.get(TARBALL)? else { bail!("advisory-db tarball not found") };
    let mut archive = tar::Archive::new(flate2::read::GzDecoder::new(&bytes[..]));
    let mut out: BTreeMap<String, Vec<Advisory>> = BTreeMap::new();
    for entry in archive.entries()? {
        let mut entry = entry?;
        let path = entry.path()?.to_string_lossy().into_owned();
        // advisory-db-main/crates/<name>/RUSTSEC-YYYY-NNNN.md
        let parts: Vec<&str> = path.split('/').collect();
        if parts.len() != 4 || parts[1] != "crates" || !parts[3].starts_with("RUSTSEC-") || !parts[3].ends_with(".md") {
            continue;
        }
        let mut text = String::new();
        entry.read_to_string(&mut text)?;
        if let Some(a) = parse(&text).with_context(|| format!("parsing {path}"))? {
            out.entry(a.package.clone()).or_default().push(a);
        }
    }
    Ok(out)
}

pub fn parse(text: &str) -> Result<Option<Advisory>> {
    let start = text.find("```toml").context("no toml front matter")? + "```toml".len();
    let end = start + text[start..].find("```").context("unterminated front matter")?;
    let front: Front = toml::from_str(&text[start..end])?;
    if front.advisory.withdrawn.is_some() {
        return Ok(None);
    }
    let body = &text[end + 3..];
    let title = body.lines().find_map(|l| l.strip_prefix("# ")).unwrap_or("").trim().replace('`', "");
    let alternatives = alternatives(body, &front.advisory.package);
    Ok(Some(Advisory {
        id: front.advisory.id,
        package: front.advisory.package,
        date: front.advisory.date,
        informational: front.advisory.informational,
        patched: front.versions.patched,
        title,
        alternatives,
    }))
}

/// Crates (and std items) an advisory recommends. Entries under an "Alternative(s)" heading
/// or "Maintained alternatives:" line are listed; crates.io links elsewhere are mentions.
fn alternatives(body: &str, package: &str) -> Vec<Alternative> {
    // Reference-style link definitions: `[label]: url`
    let defs: Vec<(String, String)> = body
        .lines()
        .filter_map(|l| {
            let l = l.trim();
            let (label, url) = l.strip_prefix('[')?.split_once("]:")?;
            Some((label.to_owned(), url.trim().to_owned()))
        })
        .collect();
    let mut out = Vec::new();
    let mut in_section = false;
    for line in body.lines() {
        let lower = line.to_ascii_lowercase();
        if line.starts_with('#') {
            in_section = lower.contains("alternative");
            continue;
        }
        if lower.contains("alternative") && line.trim_end().ends_with(':') {
            in_section = true;
            continue;
        }
        if line.trim_start().starts_with('[') && line.contains("]:") {
            continue; // a definition, resolved through its uses
        }
        let mut found: Vec<(String, String)> = links(line);
        for (label, url) in &defs {
            if line.contains(&format!("[{label}]")) && !line.contains(&format!("[{label}](")) {
                found.push((label.clone(), url.clone()));
            }
        }
        for (text, url) in found {
            let name = text.replace('`', "");
            let alt = if let Some(rest) = url.split("crates.io/crates/").nth(1).or_else(|| url.split("lib.rs/crates/").nth(1)) {
                let krate = rest.split(['/', '?', '#']).next().unwrap_or("").to_owned();
                if in_section { Alternative::Crate(krate) } else { Alternative::Mentioned(krate) }
            } else if url.contains("doc.rust-lang.org") && in_section {
                Alternative::Std(name)
            } else if in_section && url.contains("github.com") && is_crate_name(&name) {
                Alternative::Crate(name) // `[memmap2](https://github.com/…)`: verified against crates.io later
            } else {
                continue;
            };
            let named = match &alt {
                Alternative::Crate(n) | Alternative::Mentioned(n) => Some(n.as_str()),
                Alternative::Std(_) => None,
            };
            if named.is_some_and(|n| n.is_empty() || n == package) || out.contains(&alt) {
                continue;
            }
            // A listed alternative supersedes an earlier mention of the same crate.
            if let Alternative::Crate(n) = &alt {
                out.retain(|a| a != &Alternative::Mentioned(n.clone()));
            }
            if matches!(&alt, Alternative::Mentioned(n) if out.contains(&Alternative::Crate(n.clone()))) {
                continue;
            }
            out.push(alt);
        }
    }
    out
}

fn is_crate_name(s: &str) -> bool {
    !s.is_empty() && s.chars().all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
}

/// `[text](url)` pairs in a line of Markdown.
fn links(line: &str) -> Vec<(String, String)> {
    let mut out = Vec::new();
    let mut rest = line;
    while let Some(open) = rest.find('[') {
        let Some(mid) = rest[open..].find("](") else { break };
        let text = &rest[open + 1..open + mid];
        let url_start = open + mid + 2;
        let Some(close) = rest[url_start..].find(')') else { break };
        out.push((text.to_owned(), rest[url_start..url_start + close].to_owned()));
        rest = &rest[url_start + close..];
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_atty_advisory() {
        let text = r#"```toml
[advisory]
id = "RUSTSEC-2024-0375"
package = "atty"
date = "2024-09-25"
informational = "unmaintained"

[versions]
patched = []
```

# `atty` is unmaintained

The maintainer says to use std.

## Alternative(s)

- [std::io::IsTerminal](https://doc.rust-lang.org/stable/std/io/trait.IsTerminal.html) - Stable since 1.70.
- [is-terminal](https://crates.io/crates/is-terminal) - For older Rust.
"#;
        let a = parse(text).unwrap().unwrap();
        assert!(a.is_unmaintained());
        assert_eq!(a.title, "atty is unmaintained");
        assert_eq!(a.alternatives, [Alternative::Std("std::io::IsTerminal".into()), Alternative::Crate("is-terminal".into())]);
    }

    fn alts(body: &str) -> Vec<Alternative> {
        alternatives(body, "orig")
    }

    #[test]
    fn reference_links_and_plain_labels() {
        // paste: reference-style links under a heading
        let paste = "## Possible Alternative(s)\n- [`pastey`]: a fork\n\n[`pastey`]: https://crates.io/crates/pastey\n";
        assert_eq!(alts(paste), [Alternative::Crate("pastey".into())]);
        // memmap: a plain "…alternatives:" line, linking GitHub
        let memmap = "The author is unresponsive.\nMaintained alternatives:\n- [`memmap2`](https://github.com/RazrFalcon/memmap2-rs)\n";
        assert_eq!(alts(memmap), [Alternative::Crate("memmap2".into())]);
        // instant: only a mention in running text
        let instant = "the author recommends using the maintained [`web-time`] crate instead.\n\n[`web-time`]: https://crates.io/crates/web-time\n";
        assert_eq!(alts(instant), [Alternative::Mentioned("web-time".into())]);
        // the advisory's own crate isn't its successor
        let tempdir = "The [`orig`](https://crates.io/crates/orig) crate is merged into [`tempfile`](https://crates.io/crates/tempfile).";
        assert_eq!(alts(tempdir), [Alternative::Mentioned("tempfile".into())]);
    }

    #[test]
    fn withdrawn_is_skipped() {
        let text = "```toml\n[advisory]\nid = \"X\"\npackage = \"p\"\ndate = \"2020-01-01\"\nwithdrawn = \"2020-02-01\"\n```\n# t\n";
        assert!(parse(text).unwrap().is_none());
    }
}

