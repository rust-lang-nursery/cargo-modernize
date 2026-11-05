# Research: what pre-2020 Rust looks like in 2026

Method. Eight long-lived projects were checked out at their last commit of 2019 and at HEAD (Nov 2026):
rust-cookbook, ripgrep, fd, bat, hyperfine, tokei, just, alacritty. Two passes:

1. **Idiom census**: regex counts of old vs. new spellings (cookbook counted in its `.md` recipes).
2. **Tool run**: `cargo modernize --format json` on both snapshots of the seven crates.

Real before/after pairs were then mined from the same files across snapshots and from the
commit history (e.g. rust-cookbook #754, #806, #865, #872). They're attached to rules as
`[[rule.examples]]`, shown by `cargo modernize --explain <rule>`, and back the
`tests/fixtures/rusty-2019` → `rusty-2026` fixture pair.

Stabilization data comes from rust-lang/rust's **beta** branch (1.100.0 at the time):
`accepted.rs` for language features and `#[stable(since)]` in `library/` (`scripts/update-features.sh`).
Beta-only APIs used by rules (all verified in beta source): `core::mem::DropGuard`, `bool::toggle`,
`str::trim_prefix`/`trim_suffix`, `Vec::from_fn`, and the never type. From 1.99: `fs::set_times`,
`LocalKey<Cell<T>>::update`, `String::from_utf8_lossy_owned`. From 1.98: `bool::ok_or`.

## Idiom census (2019 → 2026)

```
lazy_static!                                6→0          4→0          2→0          3→0          0→0          1→0          2→0          0→0
once_cell::                                 0→0          0→0          0→0          0→7          0→1          0→4          0→0          0→0
LazyLock/OnceLock                          0→40         0→19          0→9          0→0          0→0          0→0         0→19          0→9
num_cpus::                                  8→7          1→0          1→0          0→0          0→0          0→0          0→7          0→0
available_parallelism                      0→14          0→5          0→1          0→2          0→0          0→0          0→0          0→0
extern crate                              182→0         73→3          0→0         21→0          0→0         10→9          1→0         31→0
#[macro_use]                               73→0         13→4          1→1          6→0          0→0          6→5          3→4          4→0
try!(                                       0→0          0→0          0→0          0→0          0→0          0→0          0→0          0→0
std::u32-style consts                       1→1          0→0          0→0          0→0          0→0          0→0          0→0          1→0
max_value()                                 0→0          0→0          0→0          5→0          0→0          0→0          0→0          0→0
::MAX assoc const                           0→0         2→19          0→4         0→13          0→4          0→0         2→11          5→8
error_chain                                95→0          0→0          0→0          4→0          0→0          0→0          0→0          0→0
failure::                                   0→0          0→0          0→0          0→0          0→0          0→0          0→0          0→0
structopt                                   0→0          0→0          0→0          0→0          0→0          0→0          0→0          0→0
clap::App                                   1→0          1→0          1→0          3→1          1→0          0→0          2→0          1→0
format "{}" + ident                       59→52        36→24          9→6          5→0         2→12        11→16         46→0         41→9
format inline {ident}                      2→41        6→210         7→28        0→124         7→98         3→20        7→452        1→180
match→return (let-else cand)                0→0        36→16          1→1          1→3          0→1          1→2          4→3         4→26
let-else                                    0→1         0→70          4→5          2→4          0→8          1→1         1→45        12→32
map_or(false                                0→0        11→15          0→0          3→0          0→0          2→3          0→0          0→0
is_some_and/is_ok_and                       0→0         0→12          0→5          0→7          0→3          0→0         0→22         0→22
splitn(2                                    0→0          0→0          0→0          0→0          0→0          0→0          1→1          0→0
split_once                                  0→1          0→0          0→0          0→0          0→3          0→0          0→3          0→2
as_ref().map(|s| s.as_str())                0→0          0→0          0→0          0→0          0→0          0→0          0→0          0→0
as_deref                                    0→0          0→2          0→6          0→1          0→1          0→1         0→18          0→1
mem::replace(.., Default/new())             0→0          0→0          0→0          0→0          0→0          1→0          1→1          0→1
mem::take                                   0→0          0→1          0→5          0→5          0→0          0→3          0→1          0→5
ErrorKind::Other                            2→2        18→22          0→0          0→0          5→0          0→0          0→0          1→0
io::Error::other                            0→0          0→1          0→0          0→1          0→0          0→0          0→2          0→3
trim_left/right                             0→0          1→1          0→0          0→0          0→0          0→0          0→0          0→0
matches!                                    0→5      121→156          0→3         0→35         0→12          0→1         0→53         0→19
=> true,.._ => false                        1→1        10→11          2→3          1→2          0→0          2→1          4→0          5→3
std::convert::TryFrom/TryInto imp           0→0          0→0          0→0          0→4          0→2          0→0          0→0          0→0
std::iter::FromIterator                     0→0          0→0          0→0          0→0          0→0          0→0          0→0          0→0
#[allow                                     0→1        13→15          0→3         0→10          0→4          1→6          1→9         8→19
#[expect                                    0→0          0→0          0→0          0→0          0→0          0→0          0→0          0→0
impl Default (manual)                       0→0        18→24          1→0         2→10          2→5          0→1          1→2        43→41
#[derive(.*Default                          2→2        11→25          0→1         5→15         6→14          4→6         0→26        26→69
__Nonexhaustive                             0→0         10→0          0→0          0→0          0→0          0→0          0→0          0→0
#[non_exhaustive]                           0→0          0→4          0→0          0→6          0→0          0→4          0→0          0→1
crossbeam scope                             6→6          0→0          0→0          0→0          0→0          0→0          0→0          0→0
thread::scope                               0→3          0→1          0→2          0→0          0→0          0→0          0→1          0→0
byteorder                                  14→5          0→0          0→0          0→0          0→0          0→0          0→0          0→0
Box<dyn Error                               0→6         5→16          0→0          2→1          0→0          3→4          6→7        19→22
Box<Error (bare)                            0→0          6→3          0→0          0→0          0→0          0→0          0→0          0→0
x = !x                                      0→0          0→0          0→0          0→0          0→0          0→0          0→0          0→0
strip_prefix().unwrap_or                    0→0          1→3          1→3          0→2          0→0          0→0          0→5          0→0
from_utf8_lossy().into_owned                0→0          0→2          0→0          1→0          0→0          0→0          0→0          0→2
(0..n).map().collect                        1→1          3→2          0→0          0→0          0→0          0→6          0→1          1→8
scopeguard                                  0→0          0→0          0→0          0→0          0→0          0→0          0→0          0→0
filetime                                    0→0          0→0          4→4          0→0          1→0          0→0          0→0          0→0
.with(|c| c.set(                            0→0          0→0          0→0          0→0          0→0          0→0          0→0          0→0
if c {Ok(())} else {Err                     0→0          0→0          0→0          0→0          0→0          0→0          0→0          0→3
as_any                                      0→0          0→0          0→0          0→0          0→0          0→0          0→0          0→0
spin_loop_hint                              0→0          0→0          0→0          0→0          0→0          0→0          0→0          0→0
AsciiExt                                    0→0          0→0          0→0          0→0          0→0          2→2          0→0          0→0
black_box                                   0→4         0→29          0→0          0→0          0→0          0→0          0→0          3→0
async_trait                                 0→0          0→0          0→0          0→0          0→0          0→0          0→0          0→0
```

What the projects did on their own: `lazy_static` and `atty` went to zero everywhere; inline format
args (33 → 1,153), let-else (20 → 166), `is_some_and` (0 → 71), and `matches!` became the norm;
ripgrep replaced every `__Nonexhaustive` with `#[non_exhaustive]`; bat dropped `max_value()`.

## cargo-modernize findings on the same snapshots

Rules that track a completed migration fall to zero. What remains in 2026 is real, not noise. Spot checks:
`io::Error::new(ErrorKind::Other, ..)` in ripgrep, `strip_prefix(..).unwrap_or(..)` (13, candidates
for beta `trim_prefix`), and `Err(_) => return …` matches that read better as let-else.

| rule | 2019 | 2026 |
|---|---:|---:|
| `rusty/inline-format-args` | 449 | 157 |
| `rusty/let-else` | 43 | 63 |
| `edition/extern-crate` | 37 | 15 |
| `std/atty` | 25 | 0 |
| `rusty/io-error-other` | 22 | 22 |
| `edition/2024` | 20 | 4 |
| `std/mem-prelude` | 19 | 22 |
| `edition/macro-use` | 19 | 14 |
| `rusty/matches-macro` | 19 | 10 |
| `upgrade/clap-app` | 17 | 0 |
| `rusty/is-some-and` | 16 | 18 |
| `std/lazy_static` | 15 | 0 |
| `rusty/derive-default-enum` | 12 | 3 |
| `std/error-description` | 12 | 0 |
| `std/legacy-numeric-consts` | 9 | 0 |
| `upgrade/clap-4` | 7 | 0 |
| `std/lazy_static-dep` | 7 | 0 |
| `edition/env-set-var` | 6 | 14 |
| `rusty/max-value-fn` | 5 | 0 |
| `std/atty-dep` | 5 | 0 |
| `rusty/non-exhaustive` | 4 | 0 |
| `deprecated/tempdir-path` | 4 | 0 |
| `rusty/is-none-or` | 3 | 4 |
| `deprecated/serde_yaml` | 3 | 4 |
| `upgrade/base64-engine` | 3 | 0 |
| `rusty/black-box` | 3 | 0 |
| `deprecated/ansi_term` | 3 | 0 |
| `rusty/trim-prefix` | 2 | 13 |
| `std/num_cpus` | 2 | 7 |
| `rusty/filetime` | 2 | 2 |
| `std/num_cpus-dep` | 2 | 1 |
| `upgrade/bitflags-2` | 2 | 0 |
| `deprecated/tempdir` | 2 | 0 |
| `upgrade/base64-dep` | 2 | 0 |
| `rusty/bool-ok-or` | 1 | 6 |
| `rusty/lossy-owned` | 1 | 4 |
| `rusty/split-once` | 1 | 1 |
| `rusty/trim_right` | 1 | 1 |
| `deprecated/error-chain-macro` | 1 | 0 |
| `rusty/mem-take` | 1 | 0 |
| `rusty/bool-toggle` | 1 | 0 |
| `deprecated/error-chain` | 1 | 0 |
| `deprecated/dotenv` | 1 | 0 |
| `deprecated/memmap` | 1 | 0 |
| `std/once_cell-sync-lazy` | 0 | 51 |
| `rusty/let-chains` | 0 | 23 |
| `rusty/prelude-2021` | 0 | 7 |
| `std/once_cell-unsync-oncecell` | 0 | 6 |
| `rusty/vec-from-fn` | 0 | 5 |
| `std/once_cell-dep` | 0 | 3 |
| **total** | 811 | 480 |

False positives found during the runs, and fixed:
- let-else flagged `Err(e) => return Err(e.into())`. The `else` of a let-else can't use `e`, so the
  diverging arm must now bind nothing.
- `None` parses as an identifier pattern in syn, so uppercase identifiers are treated as variants.
- `vec-from-fn` flagged `collect::<String>()`, so it now requires `Vec` or no turbofish.

## Notes for the cookbook

`src/hardware/processor/cpu-count.md` keeps `num_cpus` on purpose (for `get_physical`).
Two recipes still *call* `crossbeam::scope`, although #754 moved `crossbeam-spawn.md` to
`std::thread::scope`: `src/concurrency/thread/crossbeam-spsc.md:19` and
`src/concurrency/thread/crossbeam-complex.md:38` (they can keep `crossbeam_channel` for the channels).
