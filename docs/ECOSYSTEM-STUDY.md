# Ecosystem study: what the community recommended in 2015, 2018, 2021 and 2024, and how `cargo modernize` fares on that code

Run on 2026-11-09 with `cargo-modernize` at the `feature/cargo-modernize-lsp` branch plus the fixes listed in
[What changed because of this study](#what-changed-because-of-this-study). Scripts and raw data are in
[`docs/research/`](research/).

## Questions

1. What did the Rust community recommend in each of four years, and how much of it does `cargo modernize` know about today?
2. Take open-source code from those lists as it stood at the end of each year. How often does the tool find something, and what does it find?
3. Where does the tool fail to detect things it should?

## Method

**Recommendations.** Dated git snapshots (last commit before 31 Dec of the year) of:

| Source | 2015 | 2018 | 2021 | 2024 | What was extracted |
|---|---|---|---|---|---|
| [awesome-rust](https://github.com/rust-unofficial/awesome-rust) | yes | yes | yes | yes | every GitHub link, with its section |
| [rust-cookbook](https://github.com/rust-lang-nursery/rust-cookbook) | not yet created (first commit 2017-02) | yes | yes | yes | the crates in its `Cargo.toml` |
| [blessed.rs](https://github.com/nicoburns/blessed-rs) | not yet created (first commit 2022-03) | not yet | not yet | yes | every recommended crate in `data/crates.json` |

So 2015 is awesome-rust only; 2021 has no blessed.rs. The "recommendation" is therefore a different instrument in
each year. That is a limit on how far the years can be compared.

**Code.** From each year's awesome-rust snapshot (sections Libraries, Applications, Development tools; one entry per repo), a
seeded random shuffle took 70 candidate repos per year (280 total). Each was cloned and checked out at its last commit before
31 Dec of that year. Of 280, 255 had a root `Cargo.toml` at that commit (the rest: no manifest at the root, no history by then, or a dead clone). The first 30 viable
repos per year in shuffle order were run: **120 runs**.

Each run was `cargo modernize --format json --fail-on off` with all built-in rules (`walk`, `rusty`, `std`, `edition`,
`rand`, `ecosystem`, …) against the repo's own manifest, so each crate used its own `edition` (almost always 2015 or 2018).

**Ground truth for recall.** For 15 idioms that a regular expression can find reliably (`try!`, `lazy_static!`, `mem::uninitialized`,
`trim_left`, `max_value()`, `std::u32::MAX`, `extern crate`, `#[macro_use]`, `ATOMIC_*_INIT`, `fn description`,
`compare_and_swap`, `== None`, `.len() == 0`, `thread::sleep_ms`, `error_chain!`) the source (comments stripped) was searched independently, and every occurrence the tool
did not report was classified by cause.

## 1. The recommendations

| | 2015 | 2018 | 2021 | 2024 |
|---|---|---|---|---|
| awesome-rust entries (Libraries section) | 152 | 367 | 458 | 601 |
| awesome-rust entries, all sections | 196 | 630 | 799 | 1142 |
| rust-cookbook crates | n/a | 45 | 52 | 52 |
| blessed.rs crates | n/a | n/a | n/a | 193 |

What the cookbook swapped (the clearest signal of *changing* advice):

- 2018 → 2021: dropped `tempdir`; added `tempfile`, `tokio`, `crossbeam-channel`, `percent-encoding`, `rand_distr`, `nalgebra`, `approx`, `unicode-segmentation`.
- 2021 → 2024: dropped `error-chain`; added `thiserror`.

**How much of this does the tool know?** Matching each year's crate names against the ecosystem data and the hand-written rules
(any rule with that `crate`, or a path rule rooted at it):

| List | Crates | Flagged today |
|---|---|---|
| cookbook 2018 | 45 | 14 (31%) |
| cookbook 2021 | 52 | 14 (27%) |
| cookbook 2024 | 52 | 14 (27%) |
| blessed.rs 2024 | 193 | 17 (9%) |
| awesome-rust libraries 2015 / 2018 / 2021 / 2024 | 152 / 367 / 458 / 601 | 8 / 20 / 22 / 20 (name match, crude) |

The same 14 cookbook crates are flagged in all three years: `ansi_term`, `base64`, `bitflags`, `byteorder`, `chrono`, `clap`, `lazy_static`, `memmap`,
`mime`, `num_cpus`, `rand`, `ring`, plus `error-chain`/`tempdir` (2018, 2021) or `thiserror`/`tokio` (2021, 2024). In other words **the
2024 cookbook still recommends 14 crates this tool has something to say about** (unmaintained, archived, or with an upgrade plan). Some of those
are a version-gated upgrade plan, not a recommendation to abandon the crate.

## 2. Hit rates on the code

| | 2015 | 2018 | 2021 | 2024 |
|---|---|---|---|---|
| runs / completed | 30 / 28 | 30 / 30 | 30 / 29 | 30 / 29 |
| scanned kLOC | 146.9 | 186.2 | 345.3 | 1204.7 |
| repos with ≥1 finding (excluding the edition nudge) | 25 (89%) | 26 (87%) | 29 (100%) | 29 (100%) |
| findings | 2,484 | 2,750 | 3,938 | 8,145 |
| findings per kLOC | 16.9 | 14.8 | 11.4 | 6.8 |
| mechanical (carries a fix) | 88% | 76% | 79% | 56% |
| `warn` or `deny` | 223 | 282 | 974 | 1,369 |
| repos with a `walk/*` hit | 22 | 24 | 26 | 25 |
| repos with a `std`, `upgrade` or `deprecated` hit | 18 | 23 | 27 | 27 |
| mean seconds per repo | 0.2 | 0.2 | 0.3 | 0.9 |

Share of findings by rule pack (edition nudge excluded):

| Pack | 2015 | 2018 | 2021 | 2024 |
|---|---|---|---|---|
| `rusty` (opinions) | 36% | 42% | 49% | 49% |
| `walk` (pre-1.50 idioms) | 24% | 41% | 16% | 15% |
| `std` | 37% | 8% | 16% | 12% |
| `ecosystem` | 1% | 4% | 3% | 14% |
| `edition` | 0% | 2% | 3% | 5% |
| `rand` | 2% | 1% | 10% | 4% |

How to read the table:

- **Hit rate is high in every year** (87–100% of repos). Nothing about being "recommended" kept the code idiomatic.
- **Density falls with year, but that is partly the sample.** The 2024 sample contains 1.2 MLOC, dominated by a few large repos (tensorflow/rust, candle, Theseus). Within a year, projects whose last commit
  was more than a year before the snapshot are older code: 2024 shows 6.5 findings/kLOC for fresh projects and 12.7 for stale ones. Median last-commit age at snapshot is 0.1–0.5 years in every year, so most of the sample is fresh.
- **Old idioms outlive their era.** `field-shorthand` (1.17) fires in 20, 15, 16 and 8 repos across the four years; `impl-lifetime-elision` (1.31) in 12, 13, 14 and 11; `len-zero` in 10, 8, 14 and 11. The `walk` pack is not only for 2015 code.
- **Each era has its signature.** `try!`: 699 findings in 17 of 28 repos in 2015 (the whole ecosystem was on it), 4 in 2024. `inline-format-args` grows 741 → 3,074 as code gets more `println!`.
  `os-raw-ffi` is concentrated in FFI crates (tensorflow-sys). `ecosystem/transitive` reaches 845 findings in 9 repos in 2024 (29 in 2018).
- **Edition gating hides the biggest 2018 migration.** 27 of 30 repos sampled for 2018 were still on edition 2015. Rules for `extern crate` (1,461 occurrences) and `#[macro_use]` (349) are gated on edition ≥ 2018, so on those repos
  the tool said nothing about them; the only edition finding was the jump to 2024.

### Precision

A sample of 3 findings per rule across 16 `walk` rules (48 findings, read in context) found them genuine, except one rule: `walk/match-unwrap-or` suggested
`unwrap_or_else(|| try!(…))` for `None => try!(…)` (2 of its 3 samples, in rust-cbor). `try!` would return from the closure, not the function.
The same flaw applied to `bool-then`, `and-then-some` and `option-map-match`. Fixed (see below). Three samples per rule is a smoke test, not a precision estimate.

Type-blindness remains: running `--dangerous` on a copy of itertools 0.14 reverted four files whose rewrites did not compile
(probable cause: `.len() == 0` rewritten to `.is_empty()` on a type with no `is_empty`; not traced further). The safety net works as designed.

## 3. Detection failures

### 3a. The run itself failed: 4 of 120 (3.3%)

All four are `cargo metadata` failures: two 2015 manifests modern cargo will not parse (`sokoban-rs`, `glfw-rs`: "failed to parse manifest"), and two workspaces with a
broken member manifest (`prost` 2021: `tests/single-include`; `cita` 2024: `cita-bft`). No findings at all for these repos.

### 3b. Files the parser could not read: 82 files in 27 of 116 repos (23%)

A file that fails to parse is skipped with a one-line warning. Share of scanned LOC lost: 9.5% (2015), 10.9% (2018), 0.9% (2021), <0.1% (2024).
By cause (first error line of each file, classified by hand-written patterns):

| Files | Cause |
|---|---|
| 36 | bare `Fn`/`FnMut`/`FnOnce`/`FnBox` trait objects (`Box<Fn(i32)>`, `&mut FnMut(..)`) |
| 13 | pre-1.0 syntax (`proc()`, `box x`, `[T, ..N]`, `\|uint\| -> T`, `use x::{mod}`) |
| ~10 | anonymous trait-method parameters (`fn f(&self, &T)`), valid in edition 2015 only |
| 7 | nightly `crate` visibility (`crate struct …`) |
| 6 | `await` as an identifier (pre-2018) |
| 10 | other (`as i32 << 24`, `extern crate "x" as y`, `#[deriving]`, `#[cfg(any(…))]` in odd position) |

About 52 of the 82 are syntax that rustc accepted in edition 2015 or 2018, so **the oldest code the walk is meant for is the code the parser rejects**. Whether `syn` 2
parses these was not tested. A token-level fallback for skipped files would be another way to still catch `try!`, `trim_left` and similar.

### 3c. Crates the tool never looked at: 1.0–16.0% of LOC

`cargo metadata --no-deps` lists workspace members only, and `source_files` skips any directory with its own `Cargo.toml`. A crate that is nested but not a member was skipped **silently**.

| | 2015 | 2018 | 2021 | 2024 |
|---|---|---|---|---|
| LOC in non-member crates | 16.0% | 7.6% | 1.0% | 4.8% |
| repos affected | 10 of 28 | 7 of 30 | 6 of 29 | 10 of 29 |

Worst cases: `ozkriff/zoc` (99.7% of its code), `arcnmx/nue` (99%), `libpnet` (87%), `gtk-rs/gtk` (40%).

### 3d. Recall against the regex ground truth

Every occurrence the tool did not report, by cause (2,741 occurrences, 15 idioms):

| Occurrences | Cause | Verdict |
|---|---|---|
| 1,810 | `extern crate` / `#[macro_use]` in a crate on edition 2015 | by design (rule needs ≥ 2018) |
| 728 | in a crate that is not a workspace member | tool gap (3c) |
| 119 | file skipped by the parser | tool gap (3b) |
| 29 | inside a `macro_rules!` body | known limit (macro bodies are not scanned) |
| 55 | unexplained by the above | see below |

Of the 55: 32 are `fn description(&self)` **definitions** inside `impl Error` blocks. `std/error-description` only matches *calls* of `.description()`/`.cause()`, so an
implementation that overrides the deprecated method goes unreported. I looked at four of the remaining 23 and all were in places the tool does not scan: a paren-delimited `macro_rules!` (nom), a `cfg_if!` block (rav1e),
and an `extern crate` inside a string literal (rls tests). The other 19 were not checked individually.

Repo-level recall for the idioms where the tool fires, summed over the four years: `lazy_static!` 25 of 29 repos, `compare_and_swap` 5 of 5, `.len() == 0` 42 of 49.
The full table is in `docs/research/data/analysis.json`.

### 3e. Gating and data limits

- Rules for `extern crate` and `#[macro_use]` stay silent on edition-2015 crates, and the only edition step offered was 2024.
- The ecosystem list is what the bundled `ecosystem.toml` says. Crate-name matching for awesome-rust entries uses the repo name as the crate name, which is wrong for some projects.
- Sampling: awesome-rust includes only projects someone chose to list, which skews toward well-known, library-heavy, and Cargo-at-the-root repos. Fresh vs. stale vintage is mixed, and each year's
  sample is a different 30 repos, so year-to-year density differences carry real sampling noise.

## What changed because of this study

| Change | Where |
|---|---|
| Warn when nested crates are not workspace members and so are not analyzed | `scan::unanalyzed_crates`, `main.rs`; test `warns_about_nested_crates_that_are_not_workspace_members` |
| Skip `walk` rewrites whose moved expression contains `?`, `try!`, `return`, `break` or `continue` | `legacy.rs` `escapes()` |
| Edition steps: `walk/edition-2018` (from 2015) and `walk/edition-2021` (from 2018) | `rules/walk.toml` |

## Open items

1. Parse failures on valid 2015/2018 code (bare `Fn()`, anonymous parameters, `await` ident): try `syn` 2, or a token-level fallback for skipped files.
2. Flag `fn description` / `fn cause` overrides, not just calls.
3. Offer the edition step before the 2024 jump (done for 2018 and 2021; the `extern crate` rules still need an `edition/2018` gate to fire after the step).
4. Scan `macro_rules!` bodies and `cfg_if!` blocks, or at least report how much was not scanned.
5. Make the 17 flagged blessed.rs crates carry per-crate guidance rather than a generic upgrade-plan note.
6. Repeat with a fixed-vintage sample (projects whose last commit is within the year) to separate "old code" from "old list".

## Reproduce

```sh
cd docs/research
# sources: git clone --filter=blob:none --no-checkout of awesome-rust, rust-cookbook, blessed-rs into src/
python3 tools/recs.py        # data/recs.json
python3 tools/coverage.py    # data/coverage.json
python3 tools/fetch.py       # clones and checks out 70 repos per year into code/<year>/
python3 tools/run.py 30      # data/results.json (not committed: 4 MB)
python3 tools/analyze.py && python3 tools/misses.py && python3 tools/coverage_loc.py
```

`tools/run.py` points at `target/debug/cargo-modernize` by absolute path; change `BIN` for your checkout.
The exact repos and commits are in `data/fetch.json`.
