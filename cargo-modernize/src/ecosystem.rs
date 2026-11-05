//! Ecosystem data from `modernize-ecosystem`: latest versions, maintenance status
//! with evidence, and suggested successors. A snapshot is built in; pass
//! `--ecosystem FILE` (or `CARGO_MODERNIZE_ECOSYSTEM`) to use a fresher one.

use anyhow::{Context, Result};
use serde::Deserialize;
use std::collections::HashMap;
use std::path::Path;

/// Statuses an `ecosystem` rule can name with `pattern`, plus the derived ones.
pub const STATUSES: &[&str] = &["unmaintained", "deprecated", "archived", "yanked", "vulnerable", "at-risk", "outdated", "transitive"];

/// Statuses worth reporting even when the crate is only a transitive dependency.
pub const HARD: &[&str] = &["unmaintained", "deprecated", "yanked"];

#[derive(Deserialize, Default)]
pub struct Ecosystem {
    #[serde(default)]
    pub generated: String,
    #[serde(default)]
    pub crates: HashMap<String, Entry>,
}

#[derive(Deserialize, Default)]
pub struct Entry {
    pub latest: Option<String>,
    pub status: Option<String>,
    #[serde(default)]
    pub evidence: Vec<String>,
    #[serde(default)]
    pub std_successors: Vec<String>,
    #[serde(default)]
    pub successors: Vec<Successor>,
}

#[derive(Deserialize)]
pub struct Successor {
    pub name: String,
    pub recommended: bool,
    #[serde(default)]
    pub why: Vec<String>,
}

pub fn load(path: Option<&Path>) -> Result<Ecosystem> {
    match path {
        Some(p) => {
            let text = std::fs::read_to_string(p).with_context(|| format!("reading {}", p.display()))?;
            toml::from_str(&text).with_context(|| format!("parsing {}", p.display()))
        }
        None => Ok(toml::from_str(include_str!("../rules/ecosystem.toml")).expect("built-in ecosystem.toml")),
    }
}

impl Entry {
    /// "Evidence: …. Recommended: …. Also consider: …" for a finding's detail line.
    pub fn advice(&self) -> String {
        let mut parts = Vec::new();
        if !self.evidence.is_empty() {
            let ev: Vec<&str> = self.evidence.iter().map(|e| e.trim_end_matches('.')).collect();
            parts.push(format!("Evidence: {}.", ev.join("; ")));
        }
        // std first: an advisory that names a std replacement means "no crate needed".
        let rec = self.std_successors.first().map(|s| format!("`{s}` (std)")).or_else(|| {
            self.successors.iter().find(|s| s.recommended).map(|s| match s.why.first() {
                Some(w) => format!("`{}` ({w})", s.name),
                None => format!("`{}`", s.name),
            })
        });
        let others: Vec<String> = self
            .std_successors
            .iter()
            .skip(1)
            .map(|s| format!("`{s}` (std)"))
            .chain(self.successors.iter().filter(|s| !(s.recommended && self.std_successors.is_empty())).map(|s| format!("`{}`", s.name)))
            .collect();
        if let Some(r) = rec {
            parts.push(format!("Recommended: {r}."));
        }
        if !others.is_empty() {
            parts.push(format!("Also consider: {}.", others.join(", ")));
        }
        parts.join(" ")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn succ(name: &str, recommended: bool) -> Successor {
        Successor { name: name.into(), recommended, why: vec!["named as an alternative in RUSTSEC-X".into()] }
    }

    #[test]
    fn std_successor_leads() {
        let e = Entry {
            evidence: vec!["RUSTSEC-1: atty is unmaintained.".into()],
            std_successors: vec!["std::io::IsTerminal".into()],
            successors: vec![succ("is-terminal", true)],
            ..Entry::default()
        };
        assert_eq!(
            e.advice(),
            "Evidence: RUSTSEC-1: atty is unmaintained. Recommended: `std::io::IsTerminal` (std). Also consider: `is-terminal`."
        );
    }

    #[test]
    fn recommended_crate_is_not_repeated() {
        let e = Entry { successors: vec![succ("dotenvy", true), succ("dotenvx", false)], ..Entry::default() };
        assert_eq!(e.advice(), "Recommended: `dotenvy` (named as an alternative in RUSTSEC-X). Also consider: `dotenvx`.");
    }
}
