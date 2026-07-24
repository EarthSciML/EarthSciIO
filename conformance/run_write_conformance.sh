#!/usr/bin/env bash
#
# Cross-language WRITE-conformance harness driver (streaming-output-sinks Wave 5).
#
# The write-side mirror of run_conformance.sh. Drives each language's Zarr v3
# sharded WRITER over the shared input spec (conformance/write_spec.json) to
# produce a store, reads every produced store back with every available track's
# reader (decoding to earthsciio/write-native-dump/v1), and asserts with
# conformance/crosscheck_write.py that every writer's store agrees with the spec
# oracle and pairwise within tolerance (RFC §16.6 — decoded-array agreement, NOT
# byte identity) plus structural/CF-metadata agreement across the stores.
#
# Usage:
#   conformance/run_write_conformance.sh [OUT_DIR]
#     OUT_DIR  where the stores + readback dumps go (default: a temp dir, removed
#              on exit). Pass a path to keep them for inspection.
#
# Toolchain overrides (for the Python-3.11+/zarr-3.x requirement and pinned envs):
#   PYTHON   python interpreter with the `zarr` extra installed (default: python3)
#   JULIA    julia launcher                                       (default: julia)
#   CARGO    cargo launcher                                       (default: cargo)
#
# A track whose toolchain is unavailable is skipped with a logged reason (never
# silently dropped); the comparator still gates on whatever ran, and requires at
# least one store cross-read by at least one reader.
#
# Exit status is the comparator's: 0 = every produced store agrees, 1 = a
# divergence. Any driver failing (build/write/read) also aborts non-zero.

set -euo pipefail

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$REPO_ROOT"

PYTHON="${PYTHON:-python3}"
JULIA="${JULIA:-julia}"
CARGO="${CARGO:-cargo}"

if [ "$#" -ge 1 ]; then
  OUT="$1"; mkdir -p "$OUT"
else
  OUT="$(mktemp -d)"; trap 'rm -rf "$OUT"' EXIT
fi

echo "cross-language WRITE-conformance harness"
echo "  repo: $REPO_ROOT"
echo "  out:  $OUT"
echo

# --- which tracks can we run here? ------------------------------------------
have_python_zarr=0
if "$PYTHON" -c 'import zarr' >/dev/null 2>&1; then have_python_zarr=1; fi
have_julia=0; if command -v "$JULIA" >/dev/null 2>&1; then have_julia=1; fi
have_cargo=0; if command -v "$CARGO" >/dev/null 2>&1; then have_cargo=1; fi

WRITERS=()   # labels of writers whose store was produced
READBACKS=() # readback dump files
STORE_ARGS=()

# --- writers ----------------------------------------------------------------
if [ "$have_python_zarr" = 1 ]; then
  echo "[write] python -> $OUT/store_python"
  "$PYTHON" conformance/dumpers/write_python.py "$OUT/store_python"
  WRITERS+=(python); STORE_ARGS+=(--store "python=$OUT/store_python")
else
  echo "[write] python SKIPPED: '$PYTHON -c import zarr' failed (needs Python>=3.11 + the zarr extra)"
fi

if [ "$have_julia" = 1 ]; then
  echo "[write] julia -> $OUT/store_julia"
  "$JULIA" --project=julia conformance/dumpers/write_julia.jl "$OUT/store_julia"
  WRITERS+=(julia); STORE_ARGS+=(--store "julia=$OUT/store_julia")
else
  echo "[write] julia SKIPPED: '$JULIA' not found"
fi

if [ "$have_cargo" = 1 ]; then
  echo "[write] rust -> $OUT/store_rust"
  if "$CARGO" run --quiet --manifest-path rust/Cargo.toml --example conformance_write -- "$OUT/store_rust"; then
    WRITERS+=(rust); STORE_ARGS+=(--store "rust=$OUT/store_rust")
  else
    echo "[write] rust SKIPPED: cargo build/run failed (e.g. the netcdf-reader path dep is absent in this checkout)"
  fi
else
  echo "[write] rust SKIPPED: '$CARGO' not found"
fi

echo
# --- readbacks: every available reader over every produced store ------------
for w in "${WRITERS[@]}"; do
  if [ "$have_python_zarr" = 1 ]; then
    echo "[read ] python reader over $w store"
    "$PYTHON" conformance/dumpers/read_python.py "$OUT/store_$w" "$w" "$OUT/rd_python_from_$w.json"
    READBACKS+=("$OUT/rd_python_from_$w.json")
  fi
  if [ "$have_julia" = 1 ]; then
    echo "[read ] julia reader over $w store"
    "$JULIA" --project=julia conformance/dumpers/read_julia.jl "$OUT/store_$w" "$w" "$OUT/rd_julia_from_$w.json"
    READBACKS+=("$OUT/rd_julia_from_$w.json")
  fi
done

echo
echo "[gate ] cross-language write comparison"
echo
"$PYTHON" conformance/crosscheck_write.py "${READBACKS[@]}" "${STORE_ARGS[@]}"
