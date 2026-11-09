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

fn modernize(dir: &Path, args: &[&str]) -> std::process::Output {
    Command::new(env!("CARGO_BIN_EXE_cargo-modernize"))
        .arg("modernize")
        .arg("--manifest-path")
        .arg(dir.join("Cargo.toml"))
        .args(args)
        .output()
        .unwrap()
}

fn read(dir: &Path, file: &str) -> String {
    std::fs::read_to_string(dir.join(file)).unwrap()
}

#[test]
fn dangerous_applies_renames_bumps_and_std_replacements() {
    let dir = copy_fixture("legacy");
    let out = modernize(&dir, &["--dangerous", "--allow-no-vcs", "--no-verify", "--format", "json"]);
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
    let src = read(&dir, "src/main.rs");
    let manifest = read(&dir, "Cargo.toml");
    // rand 0.8 → 0.10
    assert!(src.contains("use rand::distr::{Distribution, StandardUniform, Uniform};"), "{src}");
    assert!(src.contains("rand::rng()") && src.contains("rng.random()") && src.contains("rng.random_range(0..10)"));
    assert!(src.contains("use rand::RngExt;"));
    assert!(manifest.contains("rand = \"0.10\"") && manifest.contains("version = \"0.17\""), "{manifest}");
    // The cookbook migrations.
    assert!(src.contains("static WORD: LazyLock<Regex> = LazyLock::new(|| Regex::new(r\"\\w+\").unwrap());"), "{src}");
    assert!(src.contains("use std::sync::LazyLock;"));
    assert!(src.contains("std::thread::available_parallelism().map_or(1, |n| n.get())"));
    assert!(src.contains("std::thread::scope(|s| {") && src.contains("s.spawn(|| println!(\"{arr:?}\"));"), "{src}");
    assert!(!src.contains(".unwrap();\n    let mut rdr"), "crossbeam's unwrap is gone: {src}");
    // Nothing fixable is left, and the rand plan is gone after the bump.
    let v: Value = serde_json::from_slice(&out.stdout).unwrap();
    let left = rule_ids(&v);
    assert!(!left.iter().any(|i| i.starts_with("rand/")), "{left:?}");
    assert!(v["packages"][0]["findings"].as_array().unwrap().iter().all(|f| f["fix"].is_null()), "{v:#}");
}

#[test]
fn fix_applies_only_safe_rewrites() {
    let dir = copy_fixture("rusty-2019");
    let out = modernize(&dir, &["--fix", "--allow-no-vcs", "--no-verify", "--no-format", "--fail-on", "off"]);
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
    let src = read(&dir, "src/main.rs");
    for modern in [
        "matches!(c, 'a'..='z' | 'A'..='Z' | '_')",
        "let Some(x) = v.first() else { return None };",
        "name.as_deref()",
        "usize::MAX",
        "io::Error::other(\"empty line\")",
        "mem::take(&mut s.stats)",
        "\"Unable to create log file: {e}\"",
        "#[derive(Debug, Default)]",
        "#[default]",
    ] {
        assert!(src.contains(modern), "missing {modern:?} in\n{src}");
    }
    assert!(!src.contains("impl Default for ClickState"));
    // Dangerous rewrites are left for --dangerous.
    assert!(src.contains("binary.map_or(false, |b| b)"), "{src}");
    assert!(src.contains("COUNT.with(|c| c.set(c.get() + 1))"));
    assert!(read(&dir, "src/legacy_api/mod.rs").contains("__Nonexhaustive"));
}

#[test]
fn dangerous_reverts_files_that_stop_compiling() {
    let dir = copy_fixture("modernize-me");
    let out = modernize(&dir, &["--dangerous", "--allow-no-vcs", "--fail-on", "off"]);
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(out.status.success(), "{stderr}");
    assert!(stderr.contains("Reverted 1 file(s) whose rewrites didn't compile: src/broken.rs"), "{stderr}");
    assert!(stderr.contains("Verified: `cargo check` passes"), "{stderr}");
    let main = read(&dir, "src/main.rs");
    assert!(main.contains("HITS.update(|c| c + 1)") && main.contains("is_some_and(|n| n.len() > 3)") && main.contains("(len < 10).ok_or_else("), "{main}");
    assert!(!main.contains("use std::convert::TryFrom;"));
    assert!(read(&dir, "src/broken.rs").contains("r.map_or(false, |x| x > 0)"));
}

#[test]
fn refuses_to_rewrite_without_vcs() {
    let dir = copy_fixture("modernize-me");
    let before = read(&dir, "src/main.rs");
    let out = modernize(&dir, &["--fix"]);
    assert!(!out.status.success());
    assert!(String::from_utf8_lossy(&out.stderr).contains("--allow-no-vcs"));
    assert_eq!(read(&dir, "src/main.rs"), before);
}

#[test]
fn diff_writes_nothing() {
    let dir = copy_fixture("modernize-me");
    let before = read(&dir, "src/main.rs");
    let out = modernize(&dir, &["--diff"]);
    assert_eq!(out.status.code(), Some(1), "changes pending → exit 1, like `cargo fmt --check`");
    let diff = String::from_utf8_lossy(&out.stdout);
    assert!(diff.contains("--- a/src/main.rs") && diff.contains("+    matches!(c, '0'..='9')"), "{diff}");
    assert!(!diff.contains("is_some_and"), "--diff alone previews safe rewrites only: {diff}");
    assert_eq!(read(&dir, "src/main.rs"), before);
}

#[test]
fn human_output_looks_like_rustc() {
    let out = modernize(&fixture("modernize-me"), &["--fail-on", "off"]);
    let text = String::from_utf8_lossy(&out.stdout);
    assert!(text.contains("note[rusty/matches-macro]: A `match` that only returns"), "{text}");
    assert!(text.contains("  --> src/main.rs:31:5"), "{text}");
    assert!(text.contains("help: rewrite, applied by `cargo modernize --fix`"), "{text}");
    assert!(text.contains("31 -     match c {") && text.contains("31 +     matches!(c, '0'..='9')"), "{text}");
    assert!(text.contains("generated") && text.contains("run `cargo modernize --fix` to apply"), "{text}");
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

#[test]
fn walk_rules_step_forward_by_release_band() {
    let v = run_json(&fixture("walk/Cargo.toml"), &["--fail-on", "off"]);
    let found = findings(&v);
    for rule in [
        "walk/compare-and-swap",
        "walk/os-raw-ffi",
        "walk/default-hasher",
        "walk/random-state",
        "walk/future-prelude",
    ] {
        assert!(found.iter().any(|(r, f, _)| r == rule && f == "src/main.rs"), "missing {rule}: {found:#?}");
    }
    // `--only` selects one band at a time.
    let band = findings(&run_json(&fixture("walk/Cargo.toml"), &["--fail-on", "off", "--only", "1.6"]));
    assert!(!band.is_empty() && band.iter().all(|(r, _, _)| r == "walk/os-raw-ffi"), "{band:#?}");
}

#[test]
fn walk_old_idioms_are_found_and_rewritten_to_code_that_compiles() {
    let v = run_json(&fixture("walk-old/Cargo.toml"), &["--fail-on", "off", "--only", "Walk"]);
    let found = findings(&v);
    for rule in [
        "walk/len-zero", "walk/none-compare", "walk/redundant-pattern", "walk/needless-return",
        "walk/filter-next", "walk/map-flatten", "walk/while-let-loop", "walk/option-map-match",
        "walk/match-unwrap-or", "walk/cloned", "walk/copied", "walk/unwrap-or-default",
        "walk/field-shorthand", "walk/static-lifetime", "walk/bare-trait-object", "walk/or-default",
        "walk/filter-map-identity", "walk/and-then-some", "walk/impl-lifetime-elision",
        "walk/rem-euclid", "walk/clamp", "walk/bool-then", "walk/duration-zero", "walk/repeat-n",
        "walk/total-cmp", "walk/error-description-impl", "walk/error-cause-impl",
    ] {
        assert!(found.iter().any(|(r, _, _)| r == rule), "missing {rule}: {found:#?}");
    }

    // `--dangerous` rewrites everything it can, `cargo check` verifies, and nothing is reverted.
    let dir = copy_fixture("walk-old");
    let out = modernize(&dir, &["--dangerous", "--allow-no-vcs", "--only", "Walk"]);
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(!stderr.contains("Reverted"), "{stderr}");
    let src = read(&dir, "src/main.rs");
    for want in [
        "v.is_empty()", "o.is_none()", "m.get(\"k\").is_some()", ".find(|x| **x > 1)", ".flat_map(", ".flatten()",
        ".cloned()", ".copied()", "unwrap_or_default()", "Config { name, port }", "&[&str]", "dyn fmt::Display + Send",
        ".or_default()", "o.map(|x| x + 1)", "o.unwrap_or(0)", "while let Some(x) = stack.pop()", "idx.rem_euclid(len)",
        ".then(|| idx)", "Duration::ZERO", "repeat_n(0u8, 3)", "idx.clamp(0, 10)", "Token<'_>",
    ] {
        assert!(src.contains(want), "missing `{want}` in:\n{src}");
    }
    assert!(!src.contains("return x * 2") && !src.contains("fn description"), "{src}");
    assert!(src.contains("fn cause"), "cause needs a hand edit: {src}");
    let check = Command::new("cargo").arg("check").arg("--manifest-path").arg(dir.join("Cargo.toml")).output().unwrap();
    assert!(check.status.success(), "{}", String::from_utf8_lossy(&check.stderr));
    // What is left needs judgment: the rewrite is a suggestion only.
    let left = findings(&run_json(&dir.join("Cargo.toml"), &["--fail-on", "off", "--only", "Walk"]));
    // (The edition step is a manifest nudge, not a rewrite.)
    let left: Vec<_> = left.iter().map(|(r, _, _)| r.as_str()).filter(|r| !r.starts_with("walk/edition")).collect();
    assert_eq!(left, ["walk/error-cause-impl", "walk/total-cmp"], "{left:#?}");
}

#[test]
fn warns_about_nested_crates_that_are_not_workspace_members() {
    let dir = std::env::temp_dir().join(format!("cargo-modernize-nested-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(dir.join("src")).unwrap();
    std::fs::create_dir_all(dir.join("inner/src")).unwrap();
    std::fs::write(dir.join("Cargo.toml"), "[package]\nname = \"outer\"\nversion = \"0.1.0\"\nedition = \"2021\"\n\n[workspace]\n").unwrap();
    std::fs::write(dir.join("src/lib.rs"), "").unwrap();
    std::fs::write(dir.join("inner/Cargo.toml"), "[package]\nname = \"inner\"\nversion = \"0.1.0\"\nedition = \"2021\"\n").unwrap();
    std::fs::write(dir.join("inner/src/lib.rs"), "pub fn f(v: &[u8]) -> bool { v.len() == 0 }\n").unwrap();
    let out = modernize(&dir, &["--format", "json", "--fail-on", "off"]);
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(stderr.contains("1 nested crate(s) are not workspace members") && stderr.contains("inner"), "{stderr}");
}

#[test]
fn reports_how_much_source_was_not_scanned() {
    let dir = std::env::temp_dir().join(format!("cargo-modernize-coverage-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(dir.join("src")).unwrap();
    std::fs::create_dir_all(dir.join("inner/src")).unwrap();
    std::fs::write(dir.join("Cargo.toml"), "[package]\nname = \"outer\"\nversion = \"0.1.0\"\nedition = \"2018\"\n\n[workspace]\n").unwrap();
    std::fs::write(dir.join("src/lib.rs"), "pub mod old;\npub fn ok() {}\n").unwrap();
    // syn 3 cannot parse a bare `Fn(..)` trait object, so this file is skipped.
    std::fs::write(dir.join("src/old.rs"), "pub fn f(x: Box<Fn(i32)>) {}\n\n\n").unwrap();
    std::fs::write(dir.join("inner/Cargo.toml"), "[package]\nname = \"inner\"\nversion = \"0.1.0\"\nedition = \"2018\"\n").unwrap();
    std::fs::write(dir.join("inner/src/lib.rs"), "").unwrap();

    let v = run_json(&dir.join("Cargo.toml"), &["--fail-on", "off"]);
    let c = &v["coverage"];
    assert_eq!(c["files_scanned"], 1, "{c:#}");
    assert_eq!(c["files_skipped"], 1, "{c:#}");
    assert_eq!(c["lines_skipped"], 3, "{c:#}");
    assert_eq!(c["skipped"][0]["file"], "src/old.rs", "{c:#}");
    assert_eq!(c["unanalyzed_crates"][0], "inner", "{c:#}");
    assert_eq!(v["packages"][0]["coverage"]["files_skipped"], 1);

    // The human and plans views say it too.
    for format in ["plans", "human"] {
        let out = modernize(&dir, &["--format", format, "--fail-on", "off"]);
        let text = String::from_utf8_lossy(&out.stdout);
        assert!(text.contains("NOT scanned: 1 file(s)") && text.contains("NOT analyzed: 1 nested crate(s)"), "{format}: {text}");
    }
}

#[test]
fn clean_scan_still_says_what_was_scanned() {
    let v = run_json(&fixture("modern/Cargo.toml"), &[]);
    assert!(v["coverage"]["files_scanned"].as_u64().unwrap() > 0, "{v:#}");
    assert_eq!(v["coverage"]["files_skipped"], 0);
}

#[test]
fn edition_2018_step_previews_the_cleanup_and_hands_over_after_the_bump() {
    // On edition 2015: the step, plus a preview of what it will let you delete (no fix yet).
    let v = run_json(&fixture("walk-2015/Cargo.toml"), &["--fail-on", "off"]);
    let f = findings(&v);
    let rules: Vec<_> = f.iter().map(|(r, _, _)| r.as_str()).collect();
    assert!(rules.contains(&"walk/edition-2018"), "{rules:?}");
    assert_eq!(rules.iter().filter(|r| **r == "walk/extern-crate-2015").count(), 2, "log and libc, not core: {rules:?}");
    assert_eq!(rules.iter().filter(|r| **r == "walk/macro-use-2015").count(), 1, "{rules:?}");
    assert!(!rules.iter().any(|r| *r == "edition/extern-crate" || *r == "edition/macro-use"), "{rules:?}");
    let preview: Vec<_> = v["packages"][0]["findings"].as_array().unwrap().iter().filter(|x| x["rule"].as_str().unwrap().ends_with("-2015")).collect();
    assert!(preview.iter().all(|x| x["fix"].is_null() && x["level"] == "suggest"), "{preview:#?}");

    // After the bump the real rules (with fixes) take over and the preview goes away.
    let dir = copy_fixture("walk-2015");
    let manifest = read(&dir, "Cargo.toml").replace("authors", "edition = \"2018\"\nauthors");
    std::fs::write(dir.join("Cargo.toml"), manifest).unwrap();
    let rules: Vec<String> = findings(&run_json(&dir.join("Cargo.toml"), &["--fail-on", "off"])).into_iter().map(|(r, _, _)| r).collect();
    assert!(rules.iter().any(|r| r == "edition/extern-crate") && rules.iter().any(|r| r == "edition/macro-use"), "{rules:?}");
    assert!(!rules.iter().any(|r| r.ends_with("-2015") || r == "walk/edition-2018"), "{rules:?}");
}
