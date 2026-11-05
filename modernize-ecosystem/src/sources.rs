//! crates.io API, the sparse index, and GitHub.

use crate::http::{Fetched, Http};
use anyhow::Result;
use serde::Deserialize;
use std::sync::atomic::{AtomicBool, Ordering};

// ---------------------------------------------------------------- crates.io

#[derive(Debug, Clone)]
pub struct CrateInfo {
    pub name: String,
    pub latest: Option<String>,
    /// YYYY-MM-DD of the newest non-yanked release.
    pub last_release: Option<String>,
    pub recent_downloads: u64,
    pub repository: Option<String>,
    pub description: String,
    pub all_yanked: bool,
    /// e.g. the latest version is `0.9.34+deprecated`.
    pub deprecated_version: Option<String>,
}

#[derive(Deserialize)]
struct CrateResp {
    #[serde(rename = "crate")]
    krate: CrateMeta,
    #[serde(default)]
    versions: Vec<VersionMeta>,
}

#[derive(Deserialize)]
struct CrateMeta {
    name: String,
    max_stable_version: Option<String>,
    max_version: Option<String>,
    recent_downloads: Option<u64>,
    repository: Option<String>,
    description: Option<String>,
}

#[derive(Deserialize)]
struct VersionMeta {
    num: String,
    created_at: String,
    yanked: bool,
}

pub fn crate_info(http: &Http, name: &str) -> Result<Option<CrateInfo>> {
    let Some(r) = http.json::<CrateResp>(&format!("https://crates.io/api/v1/crates/{name}"))? else { return Ok(None) };
    let live: Vec<&VersionMeta> = r.versions.iter().filter(|v| !v.yanked).collect();
    let last_release = live.iter().map(|v| &v.created_at).max().map(|d| d[..10].to_owned());
    let latest = r.krate.max_stable_version.clone().or(r.krate.max_version.clone()).filter(|v| v != "0.0.0");
    let deprecated_version = live
        .iter()
        .max_by(|a, b| a.created_at.cmp(&b.created_at))
        .filter(|v| v.num.contains("deprecated"))
        .map(|v| v.num.clone());
    Ok(Some(CrateInfo {
        name: r.krate.name,
        latest,
        last_release,
        recent_downloads: r.krate.recent_downloads.unwrap_or(0),
        repository: r.krate.repository,
        description: r.krate.description.unwrap_or_default().split_whitespace().collect::<Vec<_>>().join(" "),
        all_yanked: !r.versions.is_empty() && live.is_empty(),
        deprecated_version,
    }))
}

#[derive(Deserialize)]
struct ListResp {
    crates: Vec<ListCrate>,
}

#[derive(Deserialize)]
struct ListCrate {
    name: String,
}

/// The `n` most-downloaded crates over the last 90 days.
pub fn top_crates(http: &Http, n: usize) -> Result<Vec<String>> {
    let mut out = Vec::new();
    for page in 1..=n.div_ceil(100) {
        let url = format!("https://crates.io/api/v1/crates?sort=recent-downloads&per_page=100&page={page}");
        let Some(list) = http.json::<ListResp>(&url)? else { break };
        out.extend(list.crates.into_iter().map(|c| c.name));
    }
    out.truncate(n);
    Ok(out)
}

/// Does a crate exist? Uses the sparse index, which is CDN-backed and not rate limited.
pub fn exists(http: &Http, name: &str) -> Result<bool> {
    let lower = name.to_ascii_lowercase();
    let path = match lower.len() {
        0 => return Ok(false),
        1 => format!("1/{lower}"),
        2 => format!("2/{lower}"),
        3 => format!("3/{}/{lower}", &lower[..1]),
        _ => format!("{}/{}/{lower}", &lower[..2], &lower[2..4]),
    };
    Ok(matches!(http.get(&format!("https://index.crates.io/{path}"))?, Fetched::Ok(_)))
}

// ---------------------------------------------------------------- GitHub

#[derive(Debug, Clone, Deserialize)]
pub struct Repo {
    pub full_name: String,
    pub archived: bool,
    pub pushed_at: Option<String>,
    pub open_issues_count: u64,
    pub stargazers_count: u64,
    pub default_branch: String,
    #[serde(default)]
    pub parent: Option<Parent>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct Parent {
    pub full_name: String,
}

#[derive(Deserialize)]
struct Search {
    items: Vec<Issue>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct Issue {
    pub number: u64,
    pub title: String,
}

pub enum RepoLookup {
    Found(Repo),
    Missing,
    Unknown,
}

/// GitHub access that switches itself off when rate limited, rather than failing the run.
pub struct GitHub<'a> {
    http: &'a Http,
    enabled: AtomicBool,
}

impl<'a> GitHub<'a> {
    pub fn new(http: &'a Http, enabled: bool) -> Self {
        GitHub { http, enabled: AtomicBool::new(enabled) }
    }

    pub fn enabled(&self) -> bool {
        self.enabled.load(Ordering::Relaxed)
    }

    fn json<T: serde::de::DeserializeOwned>(&self, url: &str) -> Option<Result<Option<T>>> {
        if !self.enabled() {
            return None;
        }
        match self.http.json::<T>(url) {
            Err(e) if e.to_string().contains("rate limited") => {
                eprintln!("warning: {e}; skipping further GitHub lookups");
                self.enabled.store(false, Ordering::Relaxed);
                None
            }
            r => Some(r),
        }
    }

    pub fn repo(&self, full: &str) -> RepoLookup {
        match self.json::<Repo>(&format!("https://api.github.com/repos/{full}")) {
            Some(Ok(Some(r))) => RepoLookup::Found(r),
            Some(Ok(None)) => RepoLookup::Missing,
            _ => RepoLookup::Unknown,
        }
    }

    pub fn forks(&self, full: &str) -> Vec<Repo> {
        let url = format!("https://api.github.com/repos/{full}/forks?sort=stargazers&per_page=20");
        match self.json::<Vec<Repo>>(&url) {
            Some(Ok(Some(forks))) => forks,
            _ => Vec::new(),
        }
    }

    /// Open issues asking whether the project is maintained.
    pub fn maintenance_issues(&self, full: &str) -> Vec<Issue> {
        let url = format!("https://api.github.com/search/issues?q=repo:{full}+is:issue+is:open+maintained+in:title&per_page=5");
        match self.json::<Search>(&url) {
            Some(Ok(Some(s))) => s.items,
            _ => Vec::new(),
        }
    }

    /// The crate name published from a repository's root (or a workspace member that looks like it).
    pub fn published_name(&self, repo: &Repo, like: &str) -> Option<String> {
        let raw = |path: &str| {
            let url = format!("https://raw.githubusercontent.com/{}/{}/{path}", repo.full_name, repo.default_branch);
            self.http.text(&url).ok().flatten().and_then(|t| t.parse::<toml::Table>().ok())
        };
        let root = raw("Cargo.toml")?;
        if let Some(name) = root.get("package").and_then(|p| p.get("name")).and_then(|n| n.as_str()) {
            return Some(name.to_owned());
        }
        let stem = like.replace('_', "-");
        let members = root.get("workspace")?.get("members")?.as_array()?;
        members.iter().filter_map(|m| m.as_str()).filter(|m| !m.contains('*') && m.replace('_', "-").contains(&stem)).find_map(|m| {
            raw(&format!("{m}/Cargo.toml"))?.get("package")?.get("name")?.as_str().map(str::to_owned)
        })
    }
}

/// `https://github.com/owner/repo(.git)(/tree/...)` → `owner/repo`.
pub fn github_full_name(url: &str) -> Option<String> {
    let rest = url.split("github.com/").nth(1)?;
    let mut parts = rest.split('/').filter(|s| !s.is_empty());
    let owner = parts.next()?;
    let repo = parts.next()?.trim_end_matches(".git");
    Some(format!("{owner}/{repo}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn github_urls() {
        assert_eq!(github_full_name("https://github.com/softprops/atty").as_deref(), Some("softprops/atty"));
        assert_eq!(github_full_name("https://github.com/a/b.git").as_deref(), Some("a/b"));
        assert_eq!(github_full_name("https://github.com/a/b/tree/main/crates/c").as_deref(), Some("a/b"));
        assert_eq!(github_full_name("https://gitlab.com/a/b"), None);
    }
}
