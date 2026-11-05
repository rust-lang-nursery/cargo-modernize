//! The rule database. Rules are plain TOML so the ecosystem knowledge lives in
//! data, not code: built-in packs ship in `rules/`, and projects can add or
//! override rules with `--rules FILE` or `[[rule]]` tables in a folder's `.modernize`.

use crate::project::{Package, parse_loose_version};
use anyhow::{Context, Result, bail};
use semver::{Version, VersionReq};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::LazyLock;

#[derive(
    Debug,
    Clone,
    Copy,
    PartialEq,
    Eq,
    PartialOrd,
    Ord,
    Hash,
    Deserialize,
    Serialize,
    clap::ValueEnum,
)]
#[serde(rename_all = "kebab-case")]
pub enum Category {
    /// A newer major/minor of a dependency changes APIs you use.
    Upgrade,
    /// A crate or pattern is now covered by the standard library.
    Std,
    /// A crate or API is deprecated or unmaintained.
    Deprecated,
    /// Edition migration work.
    Edition,
    /// Older idiom with a newer, clearer spelling.
    Idiom,
    /// Opinion: a newer language/library feature that may read better. Adopt if you like it.
    Rusty,
}

impl Category {
    pub fn title(self) -> &'static str {
        match self {
            Category::Upgrade => "Dependency upgrades",
            Category::Std => "Now in std",
            Category::Deprecated => "Deprecated or unmaintained",
            Category::Edition => "Edition migration",
            Category::Idiom => "Modern idioms",
            Category::Rusty => "New ways to be Rusty",
        }
    }

    pub fn default_level(self) -> Level {
        match self {
            Category::Idiom | Category::Rusty => Level::Suggest,
            _ => Level::Warn,
        }
    }
}

/// How risky a rule's auto-fix is. `safe` fixes preserve behavior and are applied by
/// `--fix`; `dangerous` ones (the default) can change behavior or break the build and
/// need `--dangerous`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Deserialize, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum FixSafety {
    Safe,
    #[default]
    Dangerous,
}

/// ESLint-style severity. `suggest` is an opinion; `off` disables the rule.
#[derive(
    Debug,
    Clone,
    Copy,
    PartialEq,
    Eq,
    PartialOrd,
    Ord,
    Hash,
    Deserialize,
    Serialize,
    clap::ValueEnum,
)]
#[serde(rename_all = "kebab-case")]
pub enum Level {
    Off,
    Suggest,
    Warn,
    Deny,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum Kind {
    /// A path in code or a `use` (prefix match on whole segments, resolved through imports).
    Path,
    /// A method call `.name()` or associated call `Type::name()`; gate with `requires_import`.
    Method,
    /// A macro invocation, by name or resolved path.
    Macro,
    /// An attribute, by name or resolved path.
    Attribute,
    /// An `extern crate` item.
    ExternCrate,
    /// The dependency itself appears in Cargo.toml.
    Dependency,
    /// The package's `edition`.
    Edition,
    /// A built-in structural detector, named by `pattern` (e.g. "let-else").
    Pattern,
    /// `#![feature(..)]` gates for features that are now stable (data from `features.toml`).
    FeatureGate,
    /// A dependency's status from the ecosystem data, named by `pattern` (e.g. "unmaintained").
    Ecosystem,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Rule {
    pub id: String,
    pub kind: Kind,
    pub category: Category,
    pub message: String,
    /// Severity; defaults by category (`rusty`/`idiom` → suggest, others → warn).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub level: Option<Level>,
    /// Detector name for `kind = "pattern"`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pattern: Option<String>,
    /// Before/after pairs, ideally harvested from real projects (`source`).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub examples: Vec<Example>,
    /// Groups findings into one upgrade recommendation, e.g. "rand → 0.10".
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub plan: Option<String>,
    /// Only applies when this package is a dependency.
    #[serde(rename = "crate", default, skip_serializing_if = "Option::is_none")]
    pub krate: Option<String>,
    /// Semver requirement the dependency's (locked) version must satisfy.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub version: Option<String>,
    /// Minimum Rust version the replacement needs.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub since_rust: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub min_edition: Option<u16>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub below_edition: Option<u16>,
    /// Path patterns; `{a,b}` expands, e.g. `std::{u32,u64}::MAX`.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub paths: Vec<String>,
    /// Names for method / macro / attribute rules.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub names: Vec<String>,
    /// The file must import something under one of these paths (keeps method rules precise).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub requires_import: Vec<String>,
    /// Names to skip (for `extern-crate`).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub exclude: Vec<String>,
    /// Auto-fix: replace the matched identifier with this text.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rename: Option<String>,
    /// Auto-fix for method rules: per-name renames (`trim_left = "trim_start"`).
    #[serde(default, skip_serializing_if = "std::collections::BTreeMap::is_empty")]
    pub renames: std::collections::BTreeMap<String, String>,
    /// Auto-fix for path rules: replace the matched path (`once_cell::sync::Lazy` → `std::sync::LazyLock`).
    /// Through an import alias, only the last segment is renamed.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub replace_with: Option<String>,
    /// Auto-fix for path rules: replace a zero-argument call of the path with this expression.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub replace_call: Option<String>,
    /// Auto-fix for path rules: drop a leading `std::`/`core::` (`std::u32::MAX` → `u32::MAX`).
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub drop_prefix: bool,
    /// Auto-fix for path rules: delete a `use` of the path when it's the item's only import.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub remove_use: bool,
    /// Whether the auto-fix is `safe` (`--fix`) or `dangerous` (`--dangerous`, the default).
    #[serde(default)]
    pub fix: FixSafety,
    /// Auto-fix for `dependency` rules: new version requirement.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub to_version: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub suggestion: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub docs: Option<String>,

    #[serde(skip)]
    pub segs: Vec<Vec<String>>,
    #[serde(skip)]
    pub import_segs: Vec<Vec<String>>,
    /// Set for rules defined in a folder's `.modernize`: they only apply below it.
    #[serde(skip)]
    pub scope: Option<PathBuf>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Example {
    pub before: String,
    pub after: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source: Option<String>,
}

/// Stabilization data for feature gates, generated from rust-lang/rust's beta branch.
pub struct Features {
    pub beta: Version,
    pub lang: HashMap<String, String>,
    pub lib: HashMap<String, String>,
}

#[derive(Deserialize)]
struct RawFeatures {
    beta: String,
    lang: HashMap<String, String>,
    lib: HashMap<String, String>,
}

pub static FEATURES: LazyLock<Features> = LazyLock::new(|| {
    let raw: RawFeatures =
        toml::from_str(include_str!("../rules/features.toml")).expect("features.toml");
    Features {
        beta: parse_loose_version(&raw.beta).expect("beta version"),
        lang: raw.lang,
        lib: raw.lib,
    }
});

impl Features {
    pub fn since(&self, name: &str) -> Option<(&str, &'static str)> {
        self.lang
            .get(name)
            .map(|v| (v.as_str(), "language"))
            .or_else(|| self.lib.get(name).map(|v| (v.as_str(), "library")))
    }
}

/// How a rule applies to a package.
pub struct Applies {
    pub note: Option<String>,
    /// The replacement is only on the beta channel; findings are capped at `suggest`.
    pub beta: bool,
    /// The replacement targets a future Rust version not yet on the installed toolchain;
    /// findings are not actionable unless nightly is installed or `--allow-future` is passed.
    pub upcoming: bool,
}

/// Availability of something stabilized in `since`, given the toolchain and MSRV.
pub fn availability(
    since: &str,
    rustc: Option<&Version>,
    msrv: Option<&Version>,
    msrv_str: &str,
) -> Option<Applies> {
    let need = parse_loose_version(since)?;
    if rustc.is_some_and(|rc| rc < &need) {
        // Not on this toolchain yet. Still worth a heads-up if it's already in beta.
        return (need <= FEATURES.beta).then(|| Applies {
            note: Some(format!("stabilized in Rust {since}, currently in beta; lands on stable with the next release")),
            beta: true,
            upcoming: true,
        });
    }
    let note = match msrv {
        Some(m) if m < &need => Some(format!(
            "needs rust-version >= {since} (crate declares {msrv_str})"
        )),
        _ => None,
    };
    Some(Applies {
        note,
        beta: false,
        upcoming: false,
    })
}

#[derive(Deserialize)]
struct RuleFile {
    #[serde(rename = "rule", default)]
    rules: Vec<Rule>,
}

const BUILTIN: &[(&str, &str)] = &[
    ("std.toml", include_str!("../rules/std.toml")),
    ("deprecated.toml", include_str!("../rules/deprecated.toml")),
    ("edition.toml", include_str!("../rules/edition.toml")),
    ("rand.toml", include_str!("../rules/rand.toml")),
    ("ring.toml", include_str!("../rules/ring.toml")),
    ("chrono.toml", include_str!("../rules/chrono.toml")),
    ("upgrades.toml", include_str!("../rules/upgrades.toml")),
    ("rusty.toml", include_str!("../rules/rusty.toml")),
    (
        "maintenance.toml",
        include_str!("../rules/maintenance.toml"),
    ),
];

pub fn builtin() -> Result<Vec<Rule>> {
    let mut all = Vec::new();
    for (name, text) in BUILTIN {
        merge(&mut all, parse(name, text)?);
    }
    Ok(all)
}

pub fn load_file(path: &Path) -> Result<Vec<Rule>> {
    let text = std::fs::read_to_string(path)
        .with_context(|| format!("reading rules {}", path.display()))?;
    parse(&path.display().to_string(), &text)
}

/// Later rules replace earlier ones with the same id, so projects can override built-ins.
pub fn merge(base: &mut Vec<Rule>, more: Vec<Rule>) {
    for r in more {
        match base.iter().position(|b| b.id == r.id) {
            Some(i) => base[i] = r,
            None => base.push(r),
        }
    }
}

pub fn parse(source: &str, text: &str) -> Result<Vec<Rule>> {
    let file: RuleFile = toml::from_str(text).with_context(|| format!("parsing rules {source}"))?;
    let mut rules = file.rules;
    for r in &mut rules {
        let ctx = || format!("rule `{}` in {source}", r.id);
        if let Some(v) = &r.version {
            VersionReq::parse(v).with_context(ctx)?;
        }
        if let Some(v) = &r.since_rust
            && parse_loose_version(v).is_none()
        {
            bail!("{}: bad since_rust `{v}`", ctx());
        }
        let needs_paths = matches!(r.kind, Kind::Path);
        let needs_names = matches!(r.kind, Kind::Method);
        if needs_paths && r.paths.is_empty() || needs_names && r.names.is_empty() {
            bail!(
                "{}: {:?} rules need {}",
                ctx(),
                r.kind,
                if needs_paths { "paths" } else { "names" }
            );
        }
        if matches!(r.kind, Kind::Ecosystem)
            && !r
                .pattern
                .as_deref()
                .is_some_and(|p| crate::ecosystem::STATUSES.contains(&p))
        {
            bail!(
                "{}: ecosystem rules need `pattern` = one of {}",
                ctx(),
                crate::ecosystem::STATUSES.join(", ")
            );
        }
        if matches!(r.kind, Kind::Pattern) {
            match r.pattern.as_deref() {
                Some(p) if crate::scan::PATTERNS.contains(&p) => {}
                other => bail!(
                    "{}: unknown pattern {other:?}; known: {}",
                    ctx(),
                    crate::scan::PATTERNS.join(", ")
                ),
            }
        }
        if matches!(r.kind, Kind::Dependency) && r.krate.is_none() {
            bail!("{}: dependency rules need `crate`", ctx());
        }
        r.segs = r
            .paths
            .iter()
            .flat_map(|p| expand(p))
            .map(|p| split(&p))
            .collect();
        r.import_segs = r.requires_import.iter().map(|p| split(p)).collect();
    }
    Ok(rules)
}

fn split(p: &str) -> Vec<String> {
    p.trim_start_matches("::")
        .split("::")
        .map(str::to_owned)
        .collect()
}

/// Brace expansion: `a::{b,c}::d` → `a::b::d`, `a::c::d`.
pub fn expand(p: &str) -> Vec<String> {
    let (Some(open), Some(close)) = (p.find('{'), p.find('}')) else {
        return vec![p.to_owned()];
    };
    let (head, tail) = (&p[..open], &p[close + 1..]);
    p[open + 1..close]
        .split(',')
        .flat_map(|alt| expand(&format!("{head}{}{tail}", alt.trim())))
        .collect()
}

impl Rule {
    pub fn default_level(&self) -> Level {
        self.level.unwrap_or(self.category.default_level())
    }

    /// `None` if the rule doesn't apply to this package.
    pub fn applies(&self, pkg: &Package, rustc: Option<&Version>) -> Option<Applies> {
        if let Some(name) = &self.krate {
            let dep = pkg.dep(name)?;
            if let (Some(req), Some(v)) = (&self.version, dep.version())
                && !VersionReq::parse(req).ok()?.matches(&v)
            {
                return None;
            }
        }
        if self.min_edition.is_some_and(|e| pkg.edition < e)
            || self.below_edition.is_some_and(|e| pkg.edition >= e)
        {
            return None;
        }
        match &self.since_rust {
            Some(since) => availability(
                since,
                rustc,
                pkg.rust_version.as_ref(),
                &pkg.rust_version_str(),
            ),
            None => Some(Applies {
                note: None,
                beta: false,
                upcoming: false,
            }),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn brace_expansion() {
        assert_eq!(
            expand("std::{u8, u16}::{MAX,MIN}"),
            [
                "std::u8::MAX",
                "std::u8::MIN",
                "std::u16::MAX",
                "std::u16::MIN"
            ]
        );
        assert_eq!(expand("a::b"), ["a::b"]);
    }

    #[test]
    fn features_data_loads() {
        assert_eq!(FEATURES.since("let_else"), Some(("1.65.0", "language")));
        assert!(FEATURES.beta >= Version::new(1, 100, 0));
    }

    #[test]
    fn builtin_rules_parse_and_ids_are_unique() {
        let mut ids = std::collections::HashSet::new();
        for (name, text) in BUILTIN {
            for r in parse(name, text).unwrap() {
                assert!(ids.insert(r.id.clone()), "duplicate rule id {}", r.id);
            }
        }
    }
}
