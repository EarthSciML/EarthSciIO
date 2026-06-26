#!/usr/bin/env bash
#
# Cross-language conformance harness driver (esio-9nb.9).
#
# Runs the Python / Julia / Rust **provider** dumpers over the committed corpus
# fixtures, fully OFFLINE, then asserts native-array equality across the three
# tracks with conformance/crosscheck.py. This is the single CI-gated entry point
# for "the same loader + the same cached blob yields the same native arrays in
# Python, Julia, and Rust" (spec/conformance.md; conformance/CROSSLANG.md).
#
# Usage:
#   conformance/run_conformance.sh [OUT_DIR]
#     OUT_DIR  where the three dumps are written (default: a temp dir, removed on
#              exit). Pass a path to keep the dumps for inspection.
#
# Exit status is the comparator's: 0 = all tracks agree, 1 = a divergence or a
# coverage regression. Any dumper failing (build/decode) also aborts non-zero.

set -euo pipefail

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$REPO_ROOT"

# Defense-in-depth: the dumpers already build offline caches rooted at the corpus
# (offline=true wins over the environment), but force the shared offline switch so
# any incidental default-cache construction also refuses the network.
export EARTHSCI_OFFLINE=1

if [ "$#" -ge 1 ]; then
  OUT="$1"
  mkdir -p "$OUT"
else
  OUT="$(mktemp -d)"
  trap 'rm -rf "$OUT"' EXIT
fi

echo "cross-language conformance harness (offline)"
echo "  repo:  $REPO_ROOT"
echo "  dumps: $OUT"
echo

echo "[1/4] Python provider dump  -> $OUT/python.json"
python3 conformance/dumpers/dump_python.py "$OUT/python.json"

echo "[2/4] Julia provider dump   -> $OUT/julia.json"
julia --project=julia conformance/dumpers/dump_julia.jl "$OUT/julia.json"

echo "[3/4] Rust provider dump    -> $OUT/rust.json"
cargo run --quiet --manifest-path rust/Cargo.toml --example conformance_dump -- "$OUT/rust.json"

echo "[4/4] Cross-language comparison"
echo
python3 conformance/crosscheck.py "$OUT/python.json" "$OUT/julia.json" "$OUT/rust.json"
