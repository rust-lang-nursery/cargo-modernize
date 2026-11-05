---
name: writing-modernize-rules
description: Add an ecosystem migration (crate upgrade, std replacement, deprecation) to cargo-modernize as a TOML rule
---

# Writing cargo-modernize rules

Rules are data. Put project rules in a `.modernize` file (they apply to that folder and below), pass `--rules FILE`, or send a PR to `cargo-modernize/rules/*.toml`. A rule whose `id` matches a built-in replaces it.

## Shape

```toml
[[rule]]
id = "rand/thread_rng"          # unique; "<crate>/<thing>" by convention
kind = "path"                   # path | method | macro | attribute | extern-crate | dependency | edition | pattern | feature-gate
category = "upgrade"            # upgrade | std | deprecated | edition | idiom | rusty
level = "warn"                  # deny | warn | suggest | off (default: by category)
plan = "rand → 0.10"            # findings with the same plan are reported together
crate = "rand"                  # only when this package is a dependency…
version = "<0.10"               # …at a (locked) version matching this semver req
paths = ["rand::thread_rng"]    # whole-segment prefix match, resolved through `use`; `{a,b}` expands
rename = "rng"                  # auto-fix: replace the matched identifier
message = "`rand::thread_rng()` was renamed `rand::rng()` (0.9)."
suggestion = "rand::rng()"      # shown when there is no auto-fix
docs = "https://rust-random.github.io/book/update.html"

[[rule.examples]]               # shown by --explain; prefer real code, and cite it
before = "let mut rng = rand::thread_rng();"
after = "let mut rng = rand::rng();"
source = "project, path/to/file.rs"
```

`kind = "pattern"` rules name a built-in structural detector with `pattern = "let-else"`. The list is `PATTERNS` in `src/scan.rs`. A new detector is Rust code in the `Scanner` visitor plus a TOML rule.

Other fields: `names` (method/macro/attribute names), `requires_import` (method rules fire only in files importing one of these paths), `since_rust` (replacement's minimum Rust; noted against `rust-version`), `min_edition` / `below_edition`, `exclude` (extern-crate names), `to_version` (auto-bump for `dependency` rules).

## Guidelines

- **Source every rule** from the crate's CHANGELOG, deprecation attributes, or upgrade guide, and cite it in `docs`. Read the published source in `~/.cargo/registry/src` when unsure.
- Don't version-gate pure renames: the old name only exists in old releases, so the rule still helps a crate that is halfway through the bump. Do gate rules on names that still exist with a different meaning (e.g. `rand::Rng` in 0.10).
- Prefer `path` rules over `method` rules. Method rules can't see types, so always add `requires_import`.
- Only add `rename` when swapping the identifier is the whole change. Signature changes get a `suggestion`.
- Opinions ("this reads better") get `category = "rusty"`, which defaults to `suggest`. Use `warn` only when the old form is deprecated or wrong.
- Gate on `since_rust`. For APIs still in beta, rules fire with a beta note and are capped at `suggest`.
- Add the old pattern to `tests/fixtures/legacy` or `rusty-2019`, and the new one to `tests/fixtures/modern` or `rusty-2026`, which must stay finding-free.
- Validate on real code before shipping: run against a few large crates and check that the hits are genuine.
