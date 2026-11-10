# Research: running `cargo modernize` on older projects (2026-11-09)

Findings, sources, and recommendations from running `cargo modernize` on 120 open-source Rust projects, taken at the end of 2015, 2018, 2021 and 2024.
The full method, tables and caveats are in [ECOSYSTEM-STUDY.md](ECOSYSTEM-STUDY.md). The scripts and data are in [research/](research/).
This page is the short version: what we found, where it came from, and what to do next.

## Summary

- **Every era's code has work in it.** 87–100% of projects had at least one finding (excluding the edition nudge). Density ran from 16.9 findings per kLOC (2015 sample) down to 6.8 (2024 sample), though the 2024 sample is dominated by a few very large repos.
- **The oldest code is the least well served.** The parser skipped 82 files in 27 of 116 projects, and about 52 of them use syntax that rustc accepted in edition 2015 or 2018. Crates nested in a repo but not listed as workspace members were skipped silently (16% of the 2015 sample's lines). Edition-gated rules say nothing on edition-2015 crates.
- **The tool's recall on the idioms it covers is good once those gaps are accounted for.** Of 2,741 regex-visible occurrences the tool did not report, 1,810 were by design (edition gating), 728 were in unanalyzed crates, 119 were in skipped files and 29 were inside `macro_rules!` bodies. 55 were unexplained, and 32 of those are `fn description` overrides, which the tool does not look for.
- **Three things were fixed during the study** (see [Done](#done)); the rest is in [Recommendations](#recommendations).

## Sources

### What the community recommended

Each is a dated git snapshot, the last commit before 31 December of the year.

| Year | Source | Commit |
|---|---|---|
| 2015 | [rust-unofficial/awesome-rust](https://github.com/rust-unofficial/awesome-rust) | `16fc3102fb3a232b6bb3e549cc2e0611a13a1622` |
| 2018 | awesome-rust | `f4c6600f0e8fddd022b304d2bfb23e8941c7edee` |
| 2018 | [rust-lang-nursery/rust-cookbook](https://github.com/rust-lang-nursery/rust-cookbook) `Cargo.toml` | `fad847ef2d0b9e39c013f6ec8fa14cb4224c4133` |
| 2021 | awesome-rust | `c2a2f591c71b169e7a340aea19bd2444c4289da8` |
| 2021 | rust-cookbook | `752b035c1ae1646c999b4d7c39c78d67215d5893` |
| 2024 | awesome-rust | `e9c477451f55968b405587c04cdac03862660489` |
| 2024 | rust-cookbook | `526cfcb964a07e357127bdc97865938fe8137ba1` |
| 2024 | [nicoburns/blessed-rs](https://github.com/nicoburns/blessed-rs) `data/crates.json` | `40b7f315d6c5aad2ecf8127095403735def58d9d` |

The cookbook's first commit is 2017-02 and blessed.rs's is 2022-03, so those two do not exist for earlier years. 2015 is awesome-rust only, and 2021 has no blessed.rs.
The lists are therefore different instruments in different years.

### The code

- 70 candidates per year, drawn by seeded shuffle from each year's awesome-rust entries (Libraries, Applications, Development tools sections).
- 255 of 280 had a root `Cargo.toml` at the year-end commit. The first 30 viable per year were run.
- Exact repos and commits: [research/data/fetch.json](research/data/fetch.json). Raw per-idiom and per-rule numbers: [research/data/analysis.json](research/data/analysis.json).

### Tool and data versions

`cargo-modernize` on the `feature/cargo-modernize-lsp` branch with the `walk` pack, the `legacy.rs` detectors and the fixes under [Done](#done), against the bundled `ecosystem.toml`.
The ecosystem verdicts below (unmaintained, archived) are that file's, not independently checked.

## Findings

### 1. The recommendations change slowly, and the tool still has opinions about them

- Cookbook crate list: 45 (2018), 52 (2021), 52 (2024). Between 2018 and 2021: `tempdir` out, `tempfile` and `tokio` among eight added. Between 2021 and 2024: `error-chain` out, `thiserror` in.
- 14 of the cookbook's crates are flagged by the tool in each of the three years (about 27–31%), including `ansi_term`, `lazy_static`, `memmap` and `ring`. Some of these are version-gated upgrade plans rather than "stop using this."
- 17 of 193 blessed.rs crates (9%) are flagged.

### 2. Hit rates

| | 2015 | 2018 | 2021 | 2024 |
|---|---|---|---|---|
| completed runs | 28 of 30 | 30 of 30 | 29 of 30 | 29 of 30 |
| repos with ≥1 finding | 25 | 26 | 29 | 29 |
| findings | 2,484 | 2,750 | 3,938 | 8,145 |
| per kLOC | 16.9 | 14.8 | 11.4 | 6.8 |
| carry an automatic fix | 88% | 76% | 79% | 56% |
| `warn` or `deny` | 223 | 282 | 974 | 1,369 |

- Each era has a signature. `try!` was 699 findings in 17 of 28 repos in 2015 and 4 in 2024. `inline-format-args` grows from 741 to 3,074.
- Pre-1.50 idioms outlive their era: field-init shorthand (1.17) appears in 20, 15, 16 and 8 repos across the four years.
- Fresh vs. stale code matters: in the 2024 sample, projects whose last commit predates the year by more than a year have about twice the density (12.7 vs 6.5 per kLOC).

### 3. Detection failures

| Failure | Size | Cause |
|---|---|---|
| Run failed | 4 of 120 | `cargo metadata` cannot load an old manifest, or a broken workspace member |
| File not parsed | 82 files, 27 repos; 9.5% / 10.9% / 0.9% / <0.1% of lines per year | bare `Fn(…)` trait objects (36), pre-1.0 syntax (13), anonymous trait parameters (~10), `crate` visibility (7), `await` as an identifier (6), other (10) |
| Crate never analyzed | 16.0% / 7.6% / 1.0% / 4.8% of lines per year | nested crate not a workspace member; skipped without a message (now warned) |
| Rule silent by design | 1,810 occurrences | `extern crate` and `#[macro_use]` rules need edition ≥ 2018 |
| Deprecated method overridden | 32 occurrences | `std/error-description` matches calls, not `fn description` definitions |
| Inside macro bodies | 29 occurrences, plus some of the unchecked remainder | `macro_rules!` and `cfg_if!` bodies are not scanned |

### 4. Precision

A smoke test of 3 findings per rule across 16 `walk` rules found them genuine except `match-unwrap-or`, which suggested `unwrap_or_else(|| try!(…))`. The same flaw applied to three other rules. Fixed.
Rules that match method names cannot see types: on itertools 0.14, `--dangerous` reverted four files whose rewrites did not compile. The safety net did its job.
This was not a precision estimate; a larger labeled sample would be needed for one.

### 5. Limits of this study

- Different recommendation sources per year (see Sources).
- Different 30 repos per year, so year-to-year differences include sampling noise. Awesome-rust skews toward well-known, library-heavy projects with a manifest at the root.
- Crate-name matching of awesome-rust entries uses the repo name, which is wrong for some projects.
- About 19 of the 55 unexplained misses were not individually checked.

## Done

1. Warn when nested crates are not workspace members and so are not analyzed (`scan::unanalyzed_crates`, with a test).
2. `walk` rewrites no longer move `?`, `try!`, `return`, `break` or `continue` into a closure (`escapes()` in `legacy.rs`).
3. `walk/edition-2018` and `walk/edition-2021` give the intermediate edition steps instead of jumping to 2024.

## Recommendations

Ordered by how much missed code each would recover, as measured here.

1. **Make the parser accept 2015/2018-era syntax.** This is the biggest single gap for the oldest code. Test whether `syn` 2 accepts bare `Fn()` types, anonymous parameters and `await` identifiers (not tried); if not, add a token-level fallback for skipped files that still catches the lexical rules (`try!`, `trim_left`, `max_value()`, `std::u32::MAX`). Always say how many lines were not scanned.
2. **Report coverage in the output.** Print "N files / X% of lines not scanned" and the unanalyzed-crate count in the summary, and in the JSON, so a clean result cannot be mistaken for a complete one.
3. **Fix the edition gate.** Add an `edition/2018` gate so `extern crate` and `#[macro_use]` findings appear as part of the 2015 → 2018 step (they cannot fire before it). That makes the walk work end to end for edition-2015 crates, 27 of the 30 in the 2018 sample.
4. **Flag `fn description` and `fn cause` overrides.** 32 occurrences in this sample; a small detector.
5. **Scan more macro bodies,** at least `macro_rules!` with paren delimiters and `cfg_if!` blocks, or report them as unscanned.
6. **Carry per-crate guidance for the flagged recommended crates.** For the 14 cookbook and 17 blessed.rs crates, say whether the finding is "abandon", "upgrade", or "informational" instead of a generic plan.
7. **Repeat the study with a fixed-vintage sample:** only projects whose last commit falls inside the year, and a larger precision sample (for example 30 findings per rule, labeled by two readers). This separates "old code" from "old list" and gives a real precision figure.
8. **Decide what to do about unloadable manifests.** 4 of 120 runs produced nothing. Cheap options: retry with `--offline` or report the failing member by name; harder: tolerate bad members.

## Reproduce

See [ECOSYSTEM-STUDY.md](ECOSYSTEM-STUDY.md#reproduce).

## Update: parse failures fixed (issue #3)

A parser swap does not help: `syn` 2 and `syn` 3 both reject all 82 files. Instead, `src/repair.rs` patches the text where the parser stops and parses again (see the README).

Re-running the same 120 projects: skipped files went from **82 to 1**. The one left is rust-sdl `src/sdl/start.rs`, whose own brackets do not balance (`#[cfg(any(...)]`), so there is nothing to repair.
Per year: 2015 27 to 0, 2018 43 to 0, 2021 9 to 1, 2024 3 to 0. Finding counts also went up, but that run includes the rules added since the study (issues #5 and #6), so they are not a clean measure of the repair.
Repaired files report positions in the original source and carry no autofix.

