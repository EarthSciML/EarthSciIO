#!/usr/bin/env bash
#
# Cross-language v2 WRITE-conformance harness (spec/writer.md §10, RFC U12).
#
# Every available writer (Python, Julia, Rust) executes every case of
# conformance/write_spec_v2.json — regions and the commit protocol, indexed
# records, statics, blobs, dtypes, resume — into its own stores; every available
# reader then reads every writer's stores back through its read-back API
# (read_record / read_static / read_blob); conformance/crosscheck_write_v2.py
# checks each writer's per-op results (which ops must fail, and how) and every
# dump against the generator oracle, BITWISE (all profiles are lossless and
# dtypes are preserved).
#
# Usage:
#   conformance/run_write_conformance_v2.sh [OUT_DIR]
#
# Toolchain overrides: PYTHON (needs the zarr>=3 extra), JULIA, CARGO. A track
# whose toolchain is missing is skipped with a logged reason; the gate still
# requires at least one writer and one reader.

set -euo pipefail

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$REPO_ROOT"

PYTHON="${PYTHON:-python3}"
JULIA="${JULIA:-julia}"
CARGO="${CARGO:-cargo}"
SPEC=conformance/write_spec_v2.json

if [ "$#" -ge 1 ]; then
  OUT="$1"; mkdir -p "$OUT"
else
  OUT="$(mktemp -d)"; trap 'rm -rf "$OUT"' EXIT
fi
OUT="$(cd "$OUT" && pwd)"

echo "v2 WRITE-conformance harness  (spec: $SPEC, out: $OUT)"

have_python=0
if "$PYTHON" -c 'import zarr, sys; sys.exit(int(zarr.__version__.split(".")[0]) < 3)' >/dev/null 2>&1; then
  have_python=1
else
  echo "[skip ] python: '$PYTHON' has no zarr>=3"
fi
have_julia=0; JULIA_ENV=""
if command -v "$JULIA" >/dev/null 2>&1; then
  have_julia=1
  JULIA_ENV="$("$JULIA" conformance/dumpers/julia_env.jl)"
else
  echo "[skip ] julia: '$JULIA' not found"
fi
have_rust=0
if command -v "$CARGO" >/dev/null 2>&1 &&
   "$CARGO" build --quiet --manifest-path rust/Cargo.toml --example conformance_write_v2; then
  have_rust=1
else
  echo "[skip ] rust: '$CARGO' missing or the build failed"
fi

WRITERS=()
if [ "$have_python" = 1 ]; then
  echo "[write] python"
  "$PYTHON" conformance/dumpers/write_v2_python.py "$SPEC" "$OUT/python"
  WRITERS+=(python)
fi
if [ "$have_julia" = 1 ]; then
  echo "[write] julia"
  "$JULIA" --project="$JULIA_ENV" conformance/dumpers/write_v2_julia.jl "$SPEC" "$OUT/julia"
  WRITERS+=(julia)
fi
if [ "$have_rust" = 1 ]; then
  echo "[write] rust"
  "$CARGO" run --quiet --manifest-path rust/Cargo.toml --example conformance_write_v2 -- \
    write "$SPEC" "$OUT/rust"
  WRITERS+=(rust)
fi

DUMPS=()
for w in "${WRITERS[@]}"; do
  if [ "$have_python" = 1 ]; then
    echo "[read ] python <- $w"
    "$PYTHON" conformance/dumpers/read_v2_python.py "$SPEC" "$OUT/$w" "$w" "$OUT/rd_python_from_$w.json"
    DUMPS+=("$OUT/rd_python_from_$w.json")
  fi
  if [ "$have_julia" = 1 ]; then
    echo "[read ] julia <- $w"
    "$JULIA" --project="$JULIA_ENV" conformance/dumpers/read_v2_julia.jl "$SPEC" "$OUT/$w" "$w" "$OUT/rd_julia_from_$w.json"
    DUMPS+=("$OUT/rd_julia_from_$w.json")
  fi
  if [ "$have_rust" = 1 ]; then
    echo "[read ] rust <- $w"
    "$CARGO" run --quiet --manifest-path rust/Cargo.toml --example conformance_write_v2 -- \
      read "$SPEC" "$OUT/$w" "$w" "$OUT/rd_rust_from_$w.json"
    DUMPS+=("$OUT/rd_rust_from_$w.json")
  fi
done

if [ "${#WRITERS[@]}" = 0 ]; then
  echo "ERROR: no writer could run" >&2
  exit 2
fi

WARGS=()
for w in "${WRITERS[@]}"; do WARGS+=(--writer "$w"); done
echo "[gate ] crosscheck"
exec "$PYTHON" conformance/crosscheck_write_v2.py --spec "$SPEC" --results "$OUT" "${WARGS[@]}" "${DUMPS[@]}"
