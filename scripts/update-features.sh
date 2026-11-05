#!/usr/bin/env bash
# Regenerates cargo-modernize/rules/features.toml from rust-lang/rust's beta branch:
# accepted language features (compiler/rustc_feature/src/accepted.rs) and
# `#[stable(feature = "...", since = "...")]` library features.
set -euo pipefail
out="$(cd "$(dirname "$0")/.." && pwd)/cargo-modernize/rules/features.toml"
work="$(mktemp -d)"
trap 'rm -rf "$work"' EXIT
cd "$work"
beta=$(curl -fsS https://raw.githubusercontent.com/rust-lang/rust/beta/src/version)
curl -fsS -o accepted.rs https://raw.githubusercontent.com/rust-lang/rust/beta/compiler/rustc_feature/src/accepted.rs
git clone -q --depth 1 --filter=blob:none --sparse -b beta https://github.com/rust-lang/rust rust
git -C rust sparse-checkout set library >/dev/null
lang() { grep -E '^[[:space:]]*\(accepted,' accepted.rs | sed -E 's/^[[:space:]]*\(accepted, ([a-z0-9_]+), "([0-9.]+)".*/\1 = "\2"/'; }
{
  echo "# Generated from rust-lang/rust@beta: compiler/rustc_feature/src/accepted.rs (lang)"
  echo "# and #[stable(feature, since)] attributes under library/ (lib). Regenerate with scripts/update-features.sh."
  echo "beta = \"$beta\""
  echo; echo "[lang]"; lang
  echo; echo "[lib]"
  grep -rhoE '#\[stable\(feature = "[a-z0-9_]+", since = "1\.[0-9]+\.[0-9]+"' rust/library \
    | sed -E 's/.*feature = "([^"]+)", since = "([^"]+)"/\1 = "\2"/' | sort -u -t' ' -k1,1 | grep -vxF -f <(lang)
} > "$out"
echo "wrote $out (beta $beta)"
