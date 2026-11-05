use serde_json::Value;
use std::path::{Path, PathBuf};
use std::process::Command;

fn fixture(name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures").join(name)
}

fn run_json(manifest: &Path, extra: &[&str]) -> Value {
    let out = Command::new(env!("CARGO_BIN_EXE_cargo-modernize"))
        .arg("modernize")
        .arg("--manifest-path")
        .arg(manifest)
        .args(["--format", "json"])
        .args(extra)
        .output()
        .unwrap();
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
    serde_json::from_slice(&out.stdout).unwrap()
}

fn rule_ids(v: &Value) -> Vec<String> {
    v["packages"][0]["findings"].as_array().unwrap().iter().map(|f| f["rule"].as_str().unwrap().to_owned()).collect()
}

#[test]
fn legacy_crate_gets_upgrade_plans_and_std_replacements() {
    let ids = rule_ids(&run_json(&fixture("legacy/Cargo.toml"), &[]));
    for expected in [
        "rand/upgrade",
        "rand/thread_rng",
        "rand/gen",
        "rand/gen_range",
        "rand/distributions",
        "rand/standard",
        "rand/rng-trait",
        "ring/upgrade",
        "ring/ecdsa-rng-arg",
        "std/lazy_static",
        "std/lazy_static-dep",
        "std/num_cpus",
        "std/num_cpus-dep",
        "std/crossbeam-scope",
        "std/byteorder",
        "std/legacy-numeric-consts",
        "deprecated/error-chain",
        "deprecated/error-chain-macro",
        "deprecated/tempdir",
        "edition/extern-crate",
        "edition/macro-use",
        "edition/2024",
    ] {
        assert!(ids.iter().any(|i| i == expected), "missing {expected}; got {ids:?}");
    }
}

#[test]
fn msrv_too_low_is_noted() {
    let v = run_json(&fixture("legacy/Cargo.toml"), &["--only", "lazy_static"]);
    let f = &v["packages"][0]["findings"][0];
    assert!(f["note"].as_str().unwrap().contains("1.80"), "{f}");
}

#[test]
fn modern_crate_is_clean() {
    let v = run_json(&fixture("modern/Cargo.toml"), &[]);
    assert_eq!(rule_ids(&v), Vec::<String>::new(), "{v:#}");
}

#[test]
fn modernize_file_disables_rules_for_a_folder() {
    let dir = copy_fixture("legacy");
    std::fs::write(dir.join(".modernize"), "[rules]\n\"rand/*\" = \"off\"\n\"Edition 2024\" = \"off\"\n").unwrap();
    let ids = rule_ids(&run_json(&dir.join("Cargo.toml"), &[]));
    assert!(!ids.iter().any(|i| i.starts_with("rand/") || i == "edition/2024"), "{ids:?}");
    assert!(ids.iter().any(|i| i == "std/num_cpus"));
}

/// (rule, file, level) for every finding.
fn findings(v: &Value) -> Vec<(String, String, String)> {
    v["packages"][0]["findings"]
        .as_array()
        .unwrap()
        .iter()
        .map(|f| (f["rule"].as_str().unwrap().into(), f["file"].as_str().unwrap().into(), f["level"].as_str().unwrap().into()))
        .collect()
}

#[test]
fn rusty_2019_gets_every_opinion() {
    let v = run_json(&fixture("rusty-2019/Cargo.toml"), &["--fail-on", "off"]);
    let found = findings(&v);
    for rule in [
        "rusty/inline-format-args",
        "rusty/let-else",
        "rusty/let-chains",
        "rusty/is-some-and",
        "rusty/is-none-or",
        "rusty/as-deref",
        "rusty/bool-ok-or",
        "rusty/bool-toggle",
        "rusty/matches-macro",
        "rusty/mem-take",
        "rusty/io-error-other",
        "rusty/split-once",
        "rusty/trim-prefix",
        "rusty/lossy-owned",
        "rusty/vec-from-fn",
        "rusty/local-key-update",
        "rusty/derive-default-enum",
        "rusty/trait-upcasting",
        "rusty/max-value-fn",
        "rusty/stable-feature-gate",
        "rusty/expect-lint",
        "acme/no-max-value-in-generated",
    ] {
        assert!(found.iter().any(|(r, f, _)| r == rule && f == "src/main.rs"), "missing {rule} in src/main.rs: {found:#?}");
    }
    // Opinions default to `suggest`; beta-only APIs are capped there too.
    let level = |rule: &str| found.iter().find(|(r, _, _)| r == rule).unwrap().2.clone();
    assert_eq!(level("rusty/let-else"), "suggest");
    assert_eq!(level("rusty/trim-prefix"), "suggest");
    assert_eq!(level("rusty/max-value-fn"), "warn");
}

#[test]
fn folder_configs_cascade() {
    let found = findings(&run_json(&fixture("rusty-2019/Cargo.toml"), &["--fail-on", "off"]));
    let in_dir = |d: &str| found.iter().filter(|(_, f, _)| f.starts_with(d)).cloned().collect::<Vec<_>>();
    // legacy_api/.modernize: "rusty/*" = "off", but one rule promoted to deny.
    assert_eq!(in_dir("src/legacy_api"), [("rusty/non-exhaustive".to_owned(), "src/legacy_api/mod.rs".to_owned(), "deny".to_owned())]);
    // generated/.modernize: root = true, "*" = "off" also silences the project rule from the root.
    assert!(in_dir("src/generated").is_empty(), "{found:#?}");
}

#[test]
fn deny_level_fails_the_run() {
    let out = Command::new(env!("CARGO_BIN_EXE_cargo-modernize"))
        .args(["modernize", "--manifest-path"])
        .arg(fixture("rusty-2019/Cargo.toml"))
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(2));
}

#[test]
fn rusty_2026_is_clean() {
    let v = run_json(&fixture("rusty-2026/Cargo.toml"), &[]);
    assert_eq!(findings(&v), vec![], "{v:#}");
}

#[test]
fn explain_shows_real_examples() {
    let out = Command::new(env!("CARGO_BIN_EXE_cargo-modernize")).args(["modernize", "--explain", "std/lazy_static"]).output().unwrap();
    let text = String::from_utf8_lossy(&out.stdout);
    assert!(text.contains("rust-cookbook #865") && text.contains("LazyLock::new"), "{text}");
}

#[test]
fn fix_applies_renames_and_bumps() {
    let dir = copy_fixture("legacy");
    let v = run_json(&dir.join("Cargo.toml"), &["--fix"]);
    let src = std::fs::read_to_string(dir.join("src/main.rs")).unwrap();
    let manifest = std::fs::read_to_string(dir.join("Cargo.toml")).unwrap();
    assert!(src.contains("use rand::distr::{Distribution, StandardUniform, Uniform};"), "{src}");
    assert!(src.contains("rand::rng()") && src.contains("rng.random()") && src.contains("rng.random_range(0..10)"));
    assert!(src.contains("use rand::RngExt;"));
    assert!(manifest.contains("rand = \"0.10\"") && manifest.contains("version = \"0.17\""), "{manifest}");
    // Nothing fixable is left, and the rand plan is gone after the bump.
    let left = rule_ids(&v);
    assert!(!left.iter().any(|i| i.starts_with("rand/")), "{left:?}");
    assert!(v["packages"][0]["findings"].as_array().unwrap().iter().all(|f| f["fix"].is_null()));
}

#[test]
fn ecosystem_data_drives_dependency_findings() {
    let dir = fixture("eco-app");
    let v = run_json(&dir.join("Cargo.toml"), &["--ecosystem", dir.join("ecosystem.toml").to_str().unwrap(), "--fail-on", "off"]);
    let fs = v["packages"][0]["findings"].as_array().unwrap();
    let get = |rule: &str| fs.iter().filter(|f| f["rule"] == rule).collect::<Vec<_>>();

    let unmaintained = get("ecosystem/unmaintained");
    assert_eq!(unmaintained.len(), 1, "{fs:#?}");
    assert_eq!(unmaintained[0]["snippet"], "oldlib = \"0.4\"");
    assert!(unmaintained[0]["detail"].as_str().unwrap().contains("Recommended: `oldlib2`"));

    let outdated = get("ecosystem/outdated");
    assert_eq!(outdated.len(), 1, "only freshlib is a major behind (midlib 2.4 is semver-compatible): {fs:#?}");
    assert!(outdated[0]["detail"].as_str().unwrap().contains("On 1.2.3; the latest release is 3.0.0"));

    let transitive = get("ecosystem/transitive");
    assert_eq!(transitive.len(), 1, "{fs:#?}");
    assert_eq!(transitive[0]["file"], "Cargo.lock");
    assert!(transitive[0]["detail"].as_str().unwrap().contains("via midlib 2.0.1 → deadlib 0.1.0"));

    // lazy_static has a hand-written rule, so the ecosystem `archived` finding is skipped.
    assert!(get("ecosystem/archived").is_empty(), "{fs:#?}");
    assert!(!get("std/lazy_static-dep").is_empty());
}

fn copy_fixture(name: &str) -> PathBuf {
    let dst = std::env::temp_dir().join(format!("cargo-modernize-{name}-{}-{:?}", std::process::id(), std::thread::current().id()));
    let _ = std::fs::remove_dir_all(&dst);
    copy_dir(&fixture(name), &dst);
    dst
}

fn copy_dir(src: &Path, dst: &Path) {
    std::fs::create_dir_all(dst).unwrap();
    for e in std::fs::read_dir(src).unwrap() {
        let e = e.unwrap();
        let to = dst.join(e.file_name());
        if e.file_type().unwrap().is_dir() {
            if e.file_name() != "target" {
                copy_dir(&e.path(), &to);
            }
        } else {
            std::fs::copy(e.path(), &to).unwrap();
        }
    }
}
