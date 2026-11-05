//! Per-folder `.modernize` files, cascading like `.editorconfig`/ESLint:
//! every `.modernize` from the workspace root down to a file's folder applies,
//! deeper files win, and `root = true` stops inheritance from above.
//!
//! ```toml
//! root = false
//!
//! [rules]                      # id, plan name, glob ("rusty/*"), or "*"
//! "rusty/*" = "off"
//! "std/num_cpus" = "deny"
//! "Edition 2024" = "suggest"
//!
//! [[rule]]                     # project rules; they only apply in this folder and below
//! id = "acme/old-client"
//! ...
//! ```

use crate::rules::{self, Level, Rule};
use anyhow::{Context, Result};
use serde::Deserialize;
use std::collections::{BTreeMap, HashMap};
use std::path::{Path, PathBuf};

pub const FILE_NAME: &str = ".modernize";

#[derive(Deserialize, Default)]
#[serde(deny_unknown_fields)]
struct Raw {
    #[serde(default)]
    root: bool,
    #[serde(default)]
    rules: BTreeMap<String, Level>,
    #[serde(default, rename = "rule")]
    defs: Vec<toml::Value>,
}

struct DirConfig {
    root: bool,
    levels: Vec<(String, Level)>,
}

#[derive(Default)]
pub struct Config {
    root: PathBuf,
    dirs: HashMap<PathBuf, DirConfig>,
}

impl Config {
    /// Finds every `.modernize` under the workspace. Rules they define are returned
    /// scoped to their folder.
    pub fn discover(ws_root: &Path) -> Result<(Config, Vec<Rule>)> {
        let mut cfg = Config { root: ws_root.to_path_buf(), dirs: HashMap::new() };
        let mut defs = Vec::new();
        let walk = walkdir::WalkDir::new(ws_root).into_iter().filter_entry(|e| {
            let name = e.file_name().to_string_lossy();
            e.depth() == 0 || !(e.file_type().is_dir() && (name == "target" || name == ".git"))
        });
        for entry in walk.filter_map(|e| e.ok()) {
            if entry.file_name() != FILE_NAME || !entry.file_type().is_file() {
                continue;
            }
            let path = entry.path();
            let dir = path.parent().unwrap().to_path_buf();
            let text = std::fs::read_to_string(path).with_context(|| format!("reading {}", path.display()))?;
            let raw: Raw = toml::from_str(&text).with_context(|| format!("parsing {}", path.display()))?;
            if !raw.defs.is_empty() {
                let as_pack = toml::to_string(&toml::Table::from_iter([("rule".to_owned(), toml::Value::Array(raw.defs))]))?;
                for mut r in rules::parse(&path.display().to_string(), &as_pack)? {
                    r.scope = Some(dir.clone());
                    defs.push(r);
                }
            }
            cfg.dirs.insert(dir, DirConfig { root: raw.root, levels: raw.rules.into_iter().collect() });
        }
        Ok((cfg, defs))
    }

    /// The effective level of `rule` for `file`.
    pub fn level(&self, rule: &Rule, file: &Path) -> Level {
        let mut chain: Vec<&DirConfig> = file
            .ancestors()
            .skip(1)
            .take_while(|d| d.starts_with(&self.root))
            .filter_map(|d| self.dirs.get(d))
            .collect();
        // Deepest first: cut at the first `root = true`, then apply shallow → deep.
        if let Some(i) = chain.iter().position(|c| c.root) {
            chain.truncate(i + 1);
        }
        chain.iter().rev().fold(rule.default_level(), |level, c| c.lookup(rule).unwrap_or(level))
    }
}

impl DirConfig {
    /// The most specific entry in this file: exact id > plan name > longest glob.
    fn lookup(&self, rule: &Rule) -> Option<Level> {
        self.levels
            .iter()
            .filter_map(|(key, level)| {
                let score = if *key == rule.id {
                    usize::MAX
                } else if rule.plan.as_deref() == Some(key.as_str()) {
                    usize::MAX - 1
                } else {
                    let prefix = key.strip_suffix('*')?;
                    rule.id.starts_with(prefix).then_some(prefix.len())?
                };
                Some((score, *level))
            })
            .max_by_key(|(score, _)| *score)
            .map(|(_, level)| level)
    }
}
