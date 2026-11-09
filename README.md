# modernize

Ecosystem-wide modernization for Rust crates. Clippy tells you about your code; `cargo modernize` tells you where the ecosystem has moved on. It works like an ESLint rule set: `warn`/`deny` for things you should act on, and `suggest` for opinions ("here's a newer way to be Rusty") that you're free to ignore.

```
▸ rand → 0.10  12 finding(s), 8 auto-fixable
  Cargo.toml:10:1  rand/upgrade
    │ rand = "0.8.5"
    = fix: 0.8.5 → 0.10
    = note: needs rust-version >= 1.85 (crate declares 1.70)
  src/main.rs:28:25  rand/thread_rng
    `rand::thread_rng()` was renamed `rand::rng()` and removed from the prelude (0.9).
    = fix: thread_rng → rng
▸ Now in std  7 finding(s)
  src/main.rs:19:1  std/lazy_static
    `lazy_static!` is replaced by `std::sync::LazyLock` (Rust 1.80).
  src/main.rs:24:15  std/num_cpus
    `num_cpus::get()` → `std::thread::available_parallelism()` (Rust 1.59).
```

| crate | what |
|---|---|
| [`cargo-modernize`](cargo-modernize) | The `cargo modernize` subcommand: static analysis with `syn`, import-aware path resolution, and TOML rule packs |
| [`modernize-ecosystem`](modernize-ecosystem) | Generator for `rules/ecosystem.toml`: latest versions, maintenance status with evidence, and successor forks (RustSec, crates.io, GitHub) |
| [`modernize-battery-pack`](modernize-battery-pack) | Battery pack: the modern replacement crates, plus agent skills for running a modernization pass and writing rules |

## Usage

`cargo modernize` reports like rustc and rewrites like rustfmt:

```sh
cargo install --path cargo-modernize
cargo modernize                       # rustc-style diagnostics with suggested rewrites
cargo modernize --diff                # preview the safe rewrites as a diff (exit 1 if any)
cargo modernize --fix                 # apply safe, behavior-preserving rewrites
cargo modernize --dangerous           # modernize everything it can, then `cargo check` and revert what broke
cargo modernize --only rand --dangerous   # one plan at a time
cargo modernize --format plans        # grouped by upgrade plan instead of rustc-style
cargo modernize --category rusty      # upgrade | std | deprecated | edition | idiom | rusty
cargo modernize --min-level warn      # hide opinions
cargo modernize --fail-on warn        # CI: exit 2 on warn or deny (default: deny only)
cargo modernize --explain rusty/let-else   # rationale + real-world before/after
cargo modernize --format json         # for tools and agents
cargo modernize --list-rules
```

```
note[rusty/matches-macro]: A `match` that only returns `true`/`false` is `matches!(x, Pat)`.
  --> src/main.rs:31:5
   |
31 |     match c {
   |     ^^^^^
   |
help: rewrite, applied by `cargo modernize --fix`
   |
31 -     match c {
32 -         '0'..='9' => true,
33 -         _ => false,
34 -     }
31 +     matches!(c, '0'..='9')
   |
```

### Two tiers of rewrites

| | applied by | examples |
|---|---|---|
| **safe** | `--fix` | inline format args, let-else, `matches!`, `mem::take`, `io::Error::other`, `as_deref`, `u32::max_value()` / `std::u32::MAX` → `u32::MAX`, `#[derive(Default)]` + `#[default]`, `trim_left` → `trim_start`, `try!` → `?`, removing stable `#![feature]` gates, `extern crate`, and imports the prelude now covers |
| **dangerous** | `--dangerous` | version bumps and the API migrations that go with them (rand 0.8 → 0.10, ring, tokio, clap), `lazy_static!` → `LazyLock`, `once_cell` → std, `num_cpus::get()` → `available_parallelism()`, `crossbeam::scope` → `std::thread::scope`, `is_some_and` / `is_none_or` (syntax can't tell `Option` from `Result`), let chains, `bool::ok_or`, `LocalKey::update`, `#[non_exhaustive]` (an API change for your users), and any rewrite that needs a newer Rust than your `rust-version` |

Rewriting follows `cargo fix`:
- It refuses a dirty working tree unless you pass `--allow-dirty` (or `--allow-no-vcs` outside git).
- It applies fixes in passes, since one rewrite can expose another. Each fix is atomic, and fixes that overlap wait for the next pass.
- It runs rustfmt on files that were rustfmt-clean before (`--no-format` to skip).
- It runs `cargo check` and reverts any file whose rewrites don't compile (`--no-verify` to skip). If the project didn't build to begin with, verification is skipped with a warning.

Rewrites are built from slices of your source, so comments inside a rewritten expression survive. A rewrite that introduces a std item adds the `use` once (e.g. `use std::sync::LazyLock;`).

## Levels and `.modernize` files

Every rule has a default level: `deny`, `warn`, `suggest` (opinion), or `off`. Upgrades,
std replacements, deprecations, and edition work default to `warn`; idioms and "rusty" rules default to
`suggest`. A few, like `rusty/expect-lint`, ship `off`, so you opt in.

Drop a `.modernize` file in any folder to configure the files in it. Files cascade like
`.editorconfig`: every `.modernize` from the workspace root down to the file's folder applies, and
the deeper one wins.

```toml
# src/legacy_api/.modernize
[rules]                         # key: rule id, plan name, glob, or "*"
"rusty/*" = "off"               # no opinions in this folder…
"rusty/non-exhaustive" = "deny" # …except this one, which fails CI here
"Edition 2024" = "suggest"
```

```toml
# src/generated/.modernize
root = true                     # ignore everything configured above this folder
[rules]
"*" = "off"
```

A `.modernize` can also define project rules (`[[rule]]` tables, same schema as the built-in packs),
which apply only to that folder and below. Findings for APIs that are only on the **beta**
channel are shown, but capped at `suggest` until they reach stable.

## How it works

1. `cargo metadata --no-deps` for packages, editions, `rust-version`, and dependency requirements, plus `Cargo.lock` for resolved versions.
2. A rule applies only when its crate is a dependency at a matching version and the replacement's `since_rust` is available. If the declared MSRV is too low, it says so instead of staying quiet.
3. Every `.rs` file is parsed with `syn`. `use` trees are flattened into an alias map, so `use rand::distributions::Standard; Standard.sample(..)` resolves to `rand::distributions::Standard`. Renamed dependencies (`dice = { package = "rand" }`) are mapped back too. Common macro bodies (`println!`, `vec!`, `assert!`) are parsed and scanned as well.
4. `--fix` applies only mechanical edits: identifier renames at exact spans, and version bumps in `Cargo.toml`, including `workspace = true` dependencies.

## Abandoned crates and successors

`rules/ecosystem.toml` is generated data, not hand-written rules. `cargo run -p modernize-ecosystem` rebuilds it, and `.github/workflows/ecosystem.yml` does that weekly and opens a PR. For every crate RustSec flags as unmaintained or unpatched, plus the top N by downloads (and anything in `--crates` / `--lock`), it records:

- **Status, with evidence**, strongest signal first:
  - a RustSec `unmaintained` advisory
  - the maintainer's own marker: a `+deprecated` release, or a description saying deprecated/unmaintained
  - every version yanked
  - the GitHub repository archived, gone, or moved
  - an unpatched vulnerability
  - `at-risk`: no release in 3 years, no commits in 2, *and* unmet demand (an open "is this maintained?" issue or 15+ open issues). Quiet alone doesn't count; plenty of finished crates never need a release.
- **Successors, scored on evidence**:
  - named in the advisory (+3) or recommended in its text (+2)
  - a GitHub fork of the original that publishes a crate (+2)
  - a description like "maintained fork of …" (+2)
  - a release in the last 18 months (+1)
  - 20% or more of recent downloads between the two (+1)
  
  The top successor is *recommended* only if the advisory endorses it or its score is 5+. Otherwise successors are listed as candidates.
- **The latest version**, for "a newer major version exists" suggestions.

`cargo modernize` reads this data offline. It uses the built-in snapshot, or a fresher file via `--ecosystem FILE` / `CARGO_MODERNIZE_ECOSYSTEM`. It reports direct dependencies with a status (`ecosystem/unmaintained`, `archived`, `deprecated`, `yanked` (deny), `vulnerable`, `at-risk` (suggest)), majors behind (`ecosystem/outdated`, suggest), and hard problems in **transitive** dependencies, with the chain from `Cargo.lock` (e.g. `syntect 3.3.0 → yaml-rust 0.4.3`). Crates with a hand-written rule (rand, ring, lazy_static, …) are skipped there, because that advice is more specific.

## Built-in rule packs (`cargo-modernize/rules/`)

- **std.toml**: lazy_static, once_cell, num_cpus, atty/is-terminal, crossbeam scoped threads, byteorder, maplit, static_assertions, array-init, memoffset, matches, async-trait, deprecated std APIs (`std::u32::MAX`, `mem::uninitialized`, `ATOMIC_*_INIT`, `Error::description`, `PanicInfo`), `addr_of!` → `&raw`, `try!`
- **rand.toml**: 0.8 → 0.9 → 0.10 (from rand's CHANGELOG)
- **ring.toml**: 0.16 → 0.17 and APIs deprecated in 0.17 (checked against ring 0.17.14 source)
- **chrono.toml**: the deprecated panicking constructors and timestamp APIs (checked against chrono 0.4 source)
- **upgrades.toml**: thiserror 2, syn 1, clap 4, bitflags 2, hyper 1, tokio 1, base64 Engine API
- **deprecated.toml**: error-chain, failure, structopt, tempdir, memmap, dotenv, ansi_term, yaml-rust, serde_yaml, proc-macro-error, instant, rustc-serialize, difference, paste, async-std
- **edition.toml**: 2018 idioms, 2024 migration, `env::set_var` becoming `unsafe`
- **rusty.toml**: opinions with the release that enabled each one, mostly with real before/after examples. Inline format args (1.58), let-else (1.65), let chains (1.88), `is_some_and` / `is_none_or`, `as_deref`, `matches!`, `mem::take`, `io::Error::other`, `split_once`, `#[derive(Default)]` + `#[default]`, `#[non_exhaustive]`, trait upcasting (1.86), associated constants (`u32::MAX`, `char::from_u32`), `std::hint::black_box`, `bool::ok_or` (1.98), `LocalKey::update` / `fs::set_times` / `from_utf8_lossy_owned` (1.99), and on beta (1.100): `trim_prefix`, `Vec::from_fn`, `bool::toggle`, `mem::DropGuard`
- **walk.toml**: small steps from Rust 1.0 to today, one plan per release band (`Walk: Rust 1.0–1.29`, `1.30–1.49`, `1.50–1.59`, …). Move a crate one band at a time with `cargo modernize --only "1.3" --diff --dangerous`, or silence the whole walk with `"walk/*" = "off"`. The old-idiom detectors (`is_empty`, field shorthand, `dyn`, `or_default`, `while let`, `.then`, `clamp`, …) live in `src/legacy.rs`; the rest are data-only rules for std moves and dependencies.
- **maintenance.toml**: the rules that turn ecosystem data into findings, with their levels and plans
- **ecosystem.toml**: generated data (see above)
- **features.toml**: every stabilized language and library feature gate, up to the current beta, generated by `scripts/update-features.sh`. A `#![feature(x)]` for a stable feature is flagged.

The rules were researched against 2019 vs 2026 snapshots of eight long-lived projects. See [docs/RESEARCH.md](docs/RESEARCH.md). [docs/ECOSYSTEM-STUDY.md](docs/ECOSYSTEM-STUDY.md) records what the community recommended in 2015, 2018, 2021 and 2024, and the hit rates and detection failures of running the tool on 120 projects from those lists.

Add your own in a `.modernize` file (scoped to its folder) or with `--rules FILE`. See `modernize-battery-pack/skills/writing-modernize-rules`.

## Limitations

- No type information: `method` rules match by name in files that import the crate, gated by version. Structural `pattern` detectors match syntax only. For example, `rusty/is-some-and` can't tell `Option::map_or` from another type's `map_or`. That's one reason they're opinions.
- Paths, macros, and attributes are resolved precisely through imports, but not through glob imports.
- Bodies of macros that aren't expression lists or statements (e.g. `lazy_static! { static ref … }`) aren't scanned.
