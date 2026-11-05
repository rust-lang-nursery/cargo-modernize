# AGENTS.md — instructions for AI coding agents on `cargo-modernize`

This repository is **`cargo modernize`**: a `cargo` subcommand that finds Rust code the ecosystem has moved past, reports it like rustc, and rewrites it like rustfmt. It covers crate upgrade plans (e.g. rand 0.8 → 0.10), std replacements (`lazy_static!` → `LazyLock`), unmaintained crates, edition migrations, and idiom/rusty opinions.

## Workspace layout

```
modernize/                              # workspace root
  Cargo.toml                            # workspace: edition 2024, rust-version 1.85
  Cargo.lock
  README.md
  AGENTS.md                             # this file
  docs/RESEARCH.md                      # 2019-vs-2026 study of 8 long-lived projects

  cargo-modernize/                      # the `cargo modernize` subcommand (lib + binary)
    Cargo.toml
    src/main.rs                         # CLI (clap), subcommand dispatch
    src/config.rs                       # .modernize file discovery + scoped rules
    src/project.rs                      # cargo metadata + lock parsing, Package model
    src/ecosystem.rs                    # generated ecosystem data (maintenance status, successors)
    src/rules.rs                        # Rule struct, TOML parsing, built-in packs
    src/scan.rs                         # syn-based parsing, path resolution, PATTERNS
    src/rewrite.rs                      # structural detectors and span-based rewrites
    src/fix.rs                          # apply fixes in passes, cargo check verification, revert
    src/report.rs                       # human / plans / json output, --explain
    rules/*.toml                        # built-in rule packs (std, rand, ring, chrono, upgrades, deprecated, edition, rusty, maintenance, ecosystem, features)
    tests/cli.rs                        # CLI integration tests
    tests/fixtures/                     # legacy/modern fixture pairs for rule validation

  modernize-ecosystem/                  # generates rules/ecosystem.toml (latest versions, RustSec status, successors)
    Cargo.toml
    src/

  modernize-battery-pack/               # curated modern replacement crates + agent skills
    Cargo.toml
    build.rs                            # battery-pack build integration
    src/
    examples/
    skills/modernize-crate/SKILL.md     # agent skill: drive a modernization pass
    skills/writing-modernize-rules/SKILL.md  # agent skill: add a new TOML rule

  .agents/skills/                       # agent skills (mirrored from battery-pack)
    application-errors/, error-handling-basics/, find-crate-source/,
    library-errors/, modernize-crate/, rust-best-practice/, writing-modernize-rules/

  .claude/skills/                       # same skills mirrored for Claude
    application-errors/, ..., writing-modernize-rules/

  .github/workflows/ecosystem.yml       # weekly ecosystem data regeneration + PR
```

## How to build and test

```sh
# Build everything
cargo build --workspace

# Run the tool on this repo itself
cargo run -p cargo-modernize

# Run the tool on another crate
cargo run -p cargo-modernize --manifest-path /path/to/other/Cargo.toml

# Run tests
cargo test --workspace

# Run only cargo-modernize tests (includes CLI + fixtures)
cargo test -p cargo-modernize

# Regenerate ecosystem data (needs network; runs in CI weekly)
cargo run -p modernize-ecosystem
```

## Key architecture facts

### Rules are data, not code
Built-in rule packs live in `cargo-modernize/rules/*.toml`. Each rule has: `id`, `kind`, `category`, `message`, `level`, `plan`, `crate`/`version`, `paths`/`names`, `since_rust`, and optional auto-fix fields (`rename`, `replace_with`, `replace_call`, `drop_prefix`, `remove_use`, `fix` = safe/dangerous). Project rules can go in a `.modernize` file (cascades like `.editorconfig`) or be passed with `--rules FILE`.

Rule `kind` values: `path`, `method`, `macro`, `attribute`, `extern-crate`, `dependency`, `edition`, `pattern`, `feature-gate`, `ecosystem`.

`kind = "pattern"` rules name a built-in structural detector from `PATTERNS` in `src/scan.rs` (e.g. `let-else`, `inline-format-args`, `is-some-and`, `mem-take`). Detectors are Rust code in the `Scanner` visitor (`scan.rs`) plus rewrites in `rewrite.rs`.

### Levels and defaults
- `deny`/`warn`: worth acting on. Upgrades, std, deprecated, edition default to `warn`.
- `suggest`: opinion. Idiom and rusty default to `suggest`.
- `off`: disabled.
- CI: `cargo modernize --fail-on warn --min-level warn` keeps opinions out of the gate.

### Fix safety
- `safe` fixes (behavior-preserving) are applied by `--fix`.
- `dangerous` fixes (version bumps, std replacements, API migrations, anything needing newer Rust) need `--dangerous`, which runs `cargo check` afterwards and reverts files that no longer compile.

### `.modernize` config files
A `.modernize` in any folder configures that folder and below (cascades, deeper wins). Example:

```toml
root = true                                 # ignore config from ancestor folders
[rules]
"rusty/*" = "off"                           # no opinions here
"rand/*" = "deny"                           # but this one is mandatory
"Edition 2024" = "suggest"
```

Project rules (`[[rule]]` tables) can also be defined in a `.modernize` file.

## Common agent tasks and how to approach them

### Adding a new rule
1. Pick a `kind` (`path`, `method`, `pattern`, etc.) and an `id` like `crate/thing`.
2. Add the rule to the appropriate `cargo-modernize/rules/*.toml` pack, or to a `.modernize` file for project-scoped rules.
3. For `pattern` kinds, add the detector name to `PATTERNS` in `src/scan.rs` and implement it in the `Scanner` visitor; add the rewrite in `src/rewrite.rs`.
4. Add fixtures: old code in `tests/fixtures/legacy` (or `rusty-2019`), new code in `tests/fixtures/modern` (or `rusty-2026`). Fixtures must stay finding-free.
5. Validate on real code: run against a few large crates and check hits are genuine.
6. Source every rule from the crate's CHANGELOG, deprecation attributes, or upgrade guide, and cite it in `docs`.

See `modernize-battery-pack/skills/writing-modernize-rules/SKILL.md` for the full skill.

### Running a modernization pass on an external crate
1. `cargo modernize --format json > modernize.json` to survey.
2. Work one plan at a time: std replacements and unmaintained crates first, then dependency upgrades, then the edition.
3. `cargo modernize --only "<plan>" --diff --dangerous` to preview, then `--fix` or `--dangerous`.
4. `cargo check --all-targets && cargo test`. Fix fallout before moving on.
5. Drop now-unused dependencies. Commit with the plan name.

See `modernize-battery-pack/skills/modernize-crate/SKILL.md` for the full skill.

### Adding a new structural pattern detector
1. Add the name to `PATTERNS` in `src/scan.rs`.
2. Implement detection in the `Scanner` visitor (same file) — match syntax, not types.
3. Add the rewrite in `src/rewrite.rs` (span-based, preserves comments inside rewritten expressions).
4. Add a `[[rule]]` TOML entry referencing `pattern = "<name>"`.

## Conventions and gotchas

- **Edition 2024, rust-version 1.85** for the workspace. Don't raise `rust-version` on other crates without asking — it's a compatibility promise to downstream users.
- **No type information**: method rules match by name in files that import the crate, gated by version. They can't tell `Option::map_or` from another type's `map_or` — that's why they're opinions.
- **No glob-import resolution**: paths are resolved through explicit `use` imports only.
- **Macro bodies**: only expression-list or statement macros are scanned (e.g. `println!`, `vec!`, `assert!`). Bodies like `lazy_static! { static ref … }` aren't scanned.
- **`--fix` refuses a dirty working tree** unless `--allow-dirty` (or `--allow-no-vcs` outside git).
- **`--dangerous` reverts files** that don't compile after `cargo check`; read the "Reverted" line for files needing hand migration.
- **A finding with a non-null `fix` is mechanical.** A finding with only `suggestion`/`message` needs judgment.
- **Beta-only APIs**: findings for APIs on the beta channel are shown, capped at `suggest` until stable.
- **Ecosystem data is generated**: `rules/ecosystem.toml` is built by `modernize-ecosystem`, not hand-written. It records maintenance status (RustSec, archived, deprecated, yanked, vulnerable, at-risk) and scored successors.

## Relevant skill files

- `modernize-battery-pack/skills/modernize-crate/SKILL.md` — drive a modernization pass plan by plan, keeping the build green.
- `modernize-battery-pack/skills/writing-modernize-rules/SKILL.md` — teach `cargo modernize` a new migration in TOML.
- `modernize-battery-pack/skills/research-crate-modernization/SKILL.md` — research how Rust crates have modernized and compile findings for cargo-modernize rule authors.
- `.agents/skills/rust-best-practice/SKILL.md` — general Rust best practices.
- `.agents/skills/application-errors/SKILL.md` and `.agents/skills/library-errors/SKILL.md` — error handling with anyhow/thiserror.
- `.agents/skills/find-crate-source/SKILL.md` — find sources for a Rust crate from crates.io.

## Documentation

- `README.md` — full user guide: usage, levels, `.modernize` files, rule packs, limitations.
- `docs/RESEARCH.md` — the 2019-vs-2026 study behind the `rusty` rules, with real before/after examples.
