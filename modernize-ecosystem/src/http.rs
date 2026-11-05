//! A small HTTP client with an on-disk cache and per-host politeness.
//! crates.io asks crawlers for at most 1 request/second and an identifying User-Agent.

use anyhow::{Context, Result, bail};
use std::collections::HashMap;
use std::io::Read;
use std::path::PathBuf;
use std::sync::Mutex;
use std::time::{Duration, Instant, SystemTime};

pub struct Http {
    agent: ureq::Agent,
    cache: PathBuf,
    ttl: Duration,
    github_token: Option<String>,
    last: Mutex<HashMap<&'static str, Instant>>,
    pub stats: Mutex<Stats>,
}

#[derive(Default, Debug)]
pub struct Stats {
    pub fetched: usize,
    pub cached: usize,
}

pub enum Fetched {
    Ok(Vec<u8>),
    NotFound,
}

impl Http {
    pub fn new(user_agent: &str, cache: PathBuf, ttl: Duration, github_token: Option<String>) -> Result<Self> {
        std::fs::create_dir_all(&cache).with_context(|| format!("creating cache {}", cache.display()))?;
        let config = ureq::Agent::config_builder()
            .user_agent(user_agent)
            .timeout_global(Some(Duration::from_secs(60)))
            .http_status_as_error(false)
            .build();
        Ok(Http {
            agent: ureq::Agent::new_with_config(config),
            cache,
            ttl,
            github_token,
            last: Mutex::new(HashMap::new()),
            stats: Mutex::new(Stats::default()),
        })
    }

    fn cache_path(&self, url: &str) -> PathBuf {
        let key: String = url
            .trim_start_matches("https://")
            .chars()
            .map(|c| if c.is_ascii_alphanumeric() || c == '.' || c == '-' { c } else { '_' })
            .collect();
        self.cache.join(key)
    }

    /// GET with caching. 404/410 are cached too (as an empty marker file).
    pub fn get(&self, url: &str) -> Result<Fetched> {
        let path = self.cache_path(url);
        // Append, don't `with_extension`: keys contain dots ("crates.io_…").
        let mut missing = path.clone().into_os_string();
        missing.push(".404");
        let missing = PathBuf::from(missing);
        for (p, found) in [(&path, true), (&missing, false)] {
            if let Ok(meta) = std::fs::metadata(p) {
                let fresh = meta.modified().ok().and_then(|m| SystemTime::now().duration_since(m).ok()).is_some_and(|age| age < self.ttl);
                if fresh {
                    self.stats.lock().unwrap().cached += 1;
                    return Ok(if found { Fetched::Ok(std::fs::read(p)?) } else { Fetched::NotFound });
                }
            }
        }
        self.throttle(url);
        let mut req = self.agent.get(url);
        if url.starts_with("https://api.github.com/") {
            req = req.header("Accept", "application/vnd.github+json");
            if let Some(t) = &self.github_token {
                req = req.header("Authorization", &format!("Bearer {t}"));
            }
        }
        let mut resp = req.call().with_context(|| format!("GET {url}"))?;
        self.stats.lock().unwrap().fetched += 1;
        let status = resp.status().as_u16();
        if status == 404 || status == 410 || status == 451 {
            std::fs::write(&missing, b"")?;
            return Ok(Fetched::NotFound);
        }
        if status == 403 || status == 429 {
            bail!("GET {url}: HTTP {status} (rate limited; set GITHUB_TOKEN or retry later)");
        }
        if !(200..300).contains(&status) {
            bail!("GET {url}: HTTP {status}");
        }
        let mut body = Vec::new();
        resp.body_mut().as_reader().read_to_end(&mut body)?;
        std::fs::write(&path, &body)?;
        Ok(Fetched::Ok(body))
    }

    pub fn json<T: serde::de::DeserializeOwned>(&self, url: &str) -> Result<Option<T>> {
        match self.get(url)? {
            Fetched::Ok(b) => Ok(Some(serde_json::from_slice(&b).with_context(|| format!("decoding {url}"))?)),
            Fetched::NotFound => Ok(None),
        }
    }

    pub fn text(&self, url: &str) -> Result<Option<String>> {
        Ok(match self.get(url)? {
            Fetched::Ok(b) => Some(String::from_utf8_lossy(&b).into_owned()),
            Fetched::NotFound => None,
        })
    }

    fn throttle(&self, url: &str) {
        let (host, gap) = if url.starts_with("https://crates.io/") {
            ("crates.io", Duration::from_millis(1000))
        } else if url.starts_with("https://api.github.com/search/") {
            ("github-search", Duration::from_millis(2100)) // 30/min authenticated
        } else if url.starts_with("https://api.github.com/") {
            ("github", Duration::from_millis(100))
        } else {
            return;
        };
        let mut last = self.last.lock().unwrap();
        if let Some(prev) = last.get(host) {
            let wait = gap.saturating_sub(prev.elapsed());
            std::thread::sleep(wait);
        }
        last.insert(host, Instant::now());
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn not_found_markers_are_per_url() {
        let dir = std::env::temp_dir().join(format!("modernize-http-{}", std::process::id()));
        let http = Http::new("test", dir.clone(), Duration::from_secs(3600), None).unwrap();
        // Simulate a cached 404 for one crate…
        let a = http.cache_path("https://crates.io/api/v1/crates/missing");
        let mut marker = a.into_os_string();
        marker.push(".404");
        std::fs::write(&marker, b"").unwrap();
        // …which must not shadow a different, cached-present crate.
        std::fs::write(http.cache_path("https://crates.io/api/v1/crates/present"), b"{}").unwrap();
        assert!(matches!(http.get("https://crates.io/api/v1/crates/missing").unwrap(), Fetched::NotFound));
        assert!(matches!(http.get("https://crates.io/api/v1/crates/present").unwrap(), Fetched::Ok(_)));
        let _ = std::fs::remove_dir_all(dir);
    }
}
