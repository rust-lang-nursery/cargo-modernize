//! Workspace discovery via `cargo metadata`, plus Cargo.lock for resolved versions.

use anyhow::{Context, Result, bail};
use semver::{Version, VersionReq};
use serde::Deserialize;
use std::path::{Path, PathBuf};
use std::process::Command;

pub struct Workspace {
    pub root: PathBuf,
    pub packages: Vec<Package>,
    /// Cargo.lock entries (empty if there's no lockfile).
    pub lock: Vec<LockPkg>,
}

pub struct Package {
    pub name: String,
    pub version: String,
    pub root: PathBuf,
    pub manifest_path: PathBuf,
    pub workspace_manifest: PathBuf,
    pub edition: u16,
    pub rust_version: Option<Version>,
    pub deps: Vec<Dep>,
}

pub struct Dep {
    pub name: String,
    pub rename: Option<String>,
    pub req: String,
    pub resolved: Option<Version>,
}

impl Dep {
    /// The table key in Cargo.toml.
    pub fn key(&self) -> &str {
        self.rename.as_deref().unwrap_or(&self.name)
    }

    /// Locked version if known, otherwise the lowest version the requirement allows.
    pub fn version(&self) -> Option<Version> {
        self.resolved.clone().or_else(|| min_version(&self.req))
    }
}

impl Package {
    pub fn dep(&self, name: &str) -> Option<&Dep> {
        self.deps.iter().find(|d| d.name == name)
    }

    pub fn rust_version_str(&self) -> String {
        self.rust_version.as_ref().map_or("none".into(), |v| format!("{}.{}", v.major, v.minor))
    }
}

#[derive(Deserialize)]
struct Metadata {
    packages: Vec<MPackage>,
    workspace_root: PathBuf,
}

#[derive(Deserialize)]
struct MPackage {
    name: String,
    version: String,
    manifest_path: PathBuf,
    edition: String,
    rust_version: Option<String>,
    dependencies: Vec<MDep>,
}

#[derive(Deserialize)]
struct MDep {
    name: String,
    req: String,
    rename: Option<String>,
}

#[derive(Deserialize)]
struct Lock {
    #[serde(default)]
    package: Vec<LockPkg>,
}

#[derive(Deserialize)]
pub struct LockPkg {
    pub name: String,
    pub version: String,
    /// `"name"` or `"name version"` or `"name version (source)"`.
    #[serde(default)]
    pub dependencies: Vec<String>,
}

pub fn load(manifest_path: Option<&Path>) -> Result<Workspace> {
    let cargo = std::env::var("CARGO").unwrap_or_else(|_| "cargo".into());
    let mut cmd = Command::new(cargo);
    cmd.args(["metadata", "--no-deps", "--format-version", "1"]);
    if let Some(p) = manifest_path {
        cmd.arg("--manifest-path").arg(p);
    }
    let out = cmd.output().context("running `cargo metadata`")?;
    if !out.status.success() {
        bail!("`cargo metadata` failed:\n{}", String::from_utf8_lossy(&out.stderr));
    }
    let meta: Metadata = serde_json::from_slice(&out.stdout).context("parsing `cargo metadata` output")?;

    let lock: Vec<LockPkg> = std::fs::read_to_string(meta.workspace_root.join("Cargo.lock"))
        .ok()
        .and_then(|t| toml::from_str::<Lock>(&t).ok())
        .map(|l| l.package)
        .unwrap_or_default();

    let workspace_manifest = meta.workspace_root.join("Cargo.toml");
    let packages = meta
        .packages
        .into_iter()
        .map(|p| {
            let deps = p
                .dependencies
                .into_iter()
                .map(|d| {
                    let req = VersionReq::parse(&d.req).ok();
                    let resolved = lock
                        .iter()
                        .filter(|l| l.name == d.name)
                        .filter_map(|l| Version::parse(&l.version).ok())
                        .filter(|v| req.as_ref().is_none_or(|r| r.matches(v)))
                        .max();
                    Dep { name: d.name, rename: d.rename, req: d.req, resolved }
                })
                .collect();
            Package {
                root: p.manifest_path.parent().unwrap().to_path_buf(),
                edition: p.edition.parse().unwrap_or(2015),
                rust_version: p.rust_version.as_deref().and_then(parse_loose_version),
                name: p.name,
                version: p.version,
                manifest_path: p.manifest_path,
                workspace_manifest: workspace_manifest.clone(),
                deps,
            }
        })
        .collect();
    Ok(Workspace { root: meta.workspace_root, packages, lock })
}

/// Parses "1.70", "1.70.0", or "1.99.0-nightly".
pub fn parse_loose_version(s: &str) -> Option<Version> {
    let core = s.trim().split(['-', '+', ' ']).next()?;
    let mut parts = core.split('.').map(|p| p.parse::<u64>());
    let major = parts.next()?.ok()?;
    let minor = parts.next().unwrap_or(Ok(0)).ok()?;
    let patch = parts.next().unwrap_or(Ok(0)).ok()?;
    Some(Version::new(major, minor, patch))
}

fn min_version(req: &str) -> Option<Version> {
    let req = VersionReq::parse(req).ok()?;
    let c = req.comparators.first()?;
    Some(Version::new(c.major, c.minor.unwrap_or(0), c.patch.unwrap_or(0)))
}

pub fn rustc_version() -> Option<Version> {
    let rustc = std::env::var("RUSTC").unwrap_or_else(|_| "rustc".into());
    let out = Command::new(rustc).arg("--version").output().ok()?;
    let text = String::from_utf8(out.stdout).ok()?;
    parse_loose_version(text.split_whitespace().nth(1)?)
}
