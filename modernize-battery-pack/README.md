# modernize-battery-pack

Bring an existing crate up to date with the Rust ecosystem. A [battery pack](https://crates.io/crates/battery-pack) built around **`cargo modernize`**, a static analyzer that is ecosystem-wide where clippy is crate-local:

- **Upgrade plans.** "Here is rand 0.8 → 0.10 for your code": every call site that hits a renamed or removed API, grouped into one plan, with auto-fixes where the change is mechanical.
- **Now in std.** `lazy_static!` → `LazyLock`, `num_cpus` → `available_parallelism`, `crossbeam::scope` → `std::thread::scope`, `byteorder` → `from_be_bytes`, `once_cell`, `atty`, `maplit`, `memoffset`, and more. Each suggestion is gated on your `rust-version`.
- **Unmaintained crates.** `error-chain`, `failure`, `tempdir`, `memmap`, `structopt`, `dotenv`, …
- **Edition work.** 2015 idioms (`extern crate`, `#[macro_use]`) and 2024 hazards (`env::set_var`).

## Quick start

```sh
cargo install cargo-modernize
cargo modernize            # report
cargo modernize --fix      # apply renames + version bumps, then re-check
cargo bp add modernize     # add the modern replacements this pack curates
```

## What the pack adds

| feature   | crates                     | replaces                       |
|-----------|----------------------------|--------------------------------|
| (default) | `anyhow`, `thiserror`      | `error-chain`, `failure`       |
| `cli`     | `clap` (derive)            | `structopt`, clap 2/3          |
| `fs`      | `tempfile`, `memmap2`      | `tempdir`, `memmap`            |
| `random`  | `rand` 0.10                | rand 0.8 / 0.9                 |

The pack doesn't add `lazy_static`, `once_cell`, `num_cpus`, `byteorder`, or `atty`: std covers those now.

## Agent skills

- `skills/modernize-crate`: drive a modernization pass plan by plan, keeping the build green.
- `skills/writing-modernize-rules`: teach `cargo modernize` a new migration in TOML.

## License

MIT OR Apache-2.0
