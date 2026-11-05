---
name: modernize-crate
description: Bring an existing Rust crate up to date (dependency upgrade plans, std replacements, unmaintained crates, edition) using cargo-modernize, one plan at a time with the build kept green
---

# Modernize a crate

> **Prerequisites:** `cargo install cargo-modernize` (optionally `cargo bp add modernize`).

## 1. Survey

```sh
cargo modernize --format json > modernize.json
```

Each finding has a `level`: `deny`/`warn` are worth acting on; `suggest` is an opinion, so adopt it only where it reads better and don't churn a codebase for it. `cargo modernize --explain <rule>` shows the rationale and real before/after code. Findings are grouped by `plan` (e.g. `"rand → 0.10"`, `"Now in std"`, `"Unmaintained crates"`). A finding with a non-null `fix` is mechanical. A finding with only `suggestion`/`message` needs judgment. A `note` mentioning `rust-version` means the replacement needs a higher MSRV. **Ask before raising `rust-version`.** It is a compatibility promise to downstream users.

Dependency health comes from generated ecosystem data. `ecosystem/*` findings carry their evidence (advisory ids, archived repos) and a recommended successor when one is endorsed. Treat a candidate that isn't recommended as a lead to investigate, not an instruction. For transitive findings, the fix is usually upgrading the crate in the `via` chain, not adding the successor yourself.

## 2. Work one plan at a time

Order: std replacements and unmaintained crates first (small, local), then dependency upgrade plans, then the edition.

For each plan:

1. `cargo modernize --only "<plan or crate>" --diff --dangerous` to preview, then `--fix` (safe rewrites) or `--dangerous` (version bumps, std replacements, API migrations). `--dangerous` runs `cargo check` afterwards and reverts files that no longer compile. Read its "Reverted" line: those files need a hand migration.
2. Work through the remaining findings for the plan by hand, using `suggestion` and `docs`.
3. `cargo check --all-targets && cargo test`. Fix fallout before moving on.
4. Drop dependencies that are now unused, e.g. `lazy_static` once no `lazy_static!` is left.
5. Commit with the plan name in the message.

## 3. Common rewrites

```rust
// lazy_static! → LazyLock (1.80)
static RE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"\w+").unwrap());

// num_cpus::get() → std (1.59)
let n = std::thread::available_parallelism().map_or(1, |n| n.get());

// crossbeam::scope → std::thread::scope (1.63); spawn closures take no argument
std::thread::scope(|s| { s.spawn(|| work(&data)); });

// byteorder → std (1.32)
let mut b = [0; 4]; rdr.read_exact(&mut b)?; let n = u32::from_be_bytes(b);

// error_chain! → thiserror (library) / anyhow (application)
```

## 4. Edition last

`cargo fix --edition`, set `edition = "2024"`, run `cargo fix --edition-idioms`, and review every `unsafe` block it inserts (e.g. around `env::set_var`).

## Gotchas

- Method rules (`.gen()`, `.choose_multiple()`) are matched by name in files that import the crate, so check that the receiver really is that crate's type.
- Silence or promote rules per folder with a `.modernize` file: `[rules] "rand/*" = "off"`, `"rusty/let-else" = "deny"`. Use `root = true` for generated code.
- In CI, `cargo modernize --fail-on warn --min-level warn` keeps opinions out of the gate.
