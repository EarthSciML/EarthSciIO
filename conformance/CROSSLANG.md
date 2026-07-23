# Cross-language conformance harness (esio-9nb.9)

The corpus oracle ([`verify.py`](verify.py)) proves the **Python** decode is
correct. This harness proves the stronger, load-bearing claim: the **Python,
Julia, and Rust providers decode the same cached blob to the _same_ native
arrays** — run fully OFFLINE against the committed [`corpus/`](corpus), CI-gated
([`.github/workflows/conformance.yml`](../.github/workflows/conformance.yml)).

It is the executable form of the architecture decision in
[`spec/conformance.md`](../spec/conformance.md): *idiomatic-per-language against a
shared spec, with cross-language correctness proven by equal arrays on a shared
corpus — so no FFI is needed.*

```
conformance/
  dumpers/dump_python.py        # drives earthsciio.Provider          -> native-dump/v1
  dumpers/dump_julia.jl         # drives EarthSciIO.const_provider     -> native-dump/v1
  ../rust/examples/conformance_dump.rs  # drives earthsciio::Provider  -> native-dump/v1
  crosscheck.py                 # asserts equality: vs oracle, pairwise, coverage
  run_conformance.sh            # driver: run all 3 dumpers + crosscheck (the gate)
```

## Run it

```bash
./conformance/run_conformance.sh            # all 3 providers + cross-check, offline
./conformance/run_conformance.sh /tmp/dumps # keep the per-track dumps for inspection
```

Needs the three toolchains: Python (`pip install -e ".[netcdf]"`), Julia
(`julia --project=julia -e 'using Pkg; Pkg.instantiate()'`), Rust (`cargo`). Exit
0 ⇔ every decoding track agrees with the oracle and pairwise with the others.

## How it works

Each track runs its **Provider** over every corpus case — resolve the case's
`resolved_url` through an offline cache rooted at the corpus, decode with the
format reader, return native arrays — and emits them as a canonical JSON dump
(schema `earthsciio/native-dump/v1`). [`crosscheck.py`](crosscheck.py) then, per
case:

1. **vs the oracle** — each track's arrays equal the corpus `expected` arrays.
2. **pairwise** — every pair of tracks that decoded the case produced equal
   arrays (same variable/coord set, dtype, dims, shape, values).
3. **coverage** — a track that registers a reader for a case's format **must**
   have decoded it (a silently-dropped case fails); a track with no reader for
   that format may skip, and the gap is logged, never hidden.

Global gates: every case is decoded by ≥1 track, and **≥1 case is decoded by all
three** (so three-way equality is actually demonstrated, not just pairwise on
disjoint subsets).

### The dump format — `earthsciio/native-dump/v1`

```jsonc
{
  "schema": "earthsciio/native-dump/v1",
  "language": "python",                    // | "julia" | "rust"
  "provider": "earthsciio.Provider",
  "readers": ["csv", "netcdf"],            // active format names this track registers
  "cases": {
    "era5-grid-sub-tile": {
      "format": "netcdf",
      "status": "decoded",
      "variables": {
        "t2m": { "dtype": "float64", "dims": ["time","latitude","longitude"],
                 "shape": [2,3,3], "data": [282.5, /* … */, null] }
      },
      "coords": {
        "time": { "dtype": "int32", "dims": ["time"], "shape": [2], "data": [0,1],
                  "units": "hours since 2018-11-08 00:00:00", "calendar": "gregorian" }
      }
    },
    "openaq-points-slice": {               // a track with no reader for the format
      "format": "csv", "status": "skipped",
      "reason": "no active reader registered for format 'csv' in the Rust track"
    }
  }
}
```

`data` is the field flattened **row-major (C order)** per `shape` — Julia/Rust
permute their column-major / pre-flattened storage to this one order, so the
arrays line up element-for-element. A masked / `_FillValue` cell is `null`
(== NaN). Strings are verbatim. A case whose format the track can't read is
`status:"skipped"` with a `reason` — **explicit, never omitted** — so a real
coverage gap is distinguishable from a dumper bug.

## Documented tolerance (`spec/conformance.md` §4)

| read kind | comparison |
|---|---|
| CF-decoded (packed) values, any unit-affected read | `|a−b| ≤ atol + rtol·|b|`, **atol = 1e-6**, **rtol = 1e-9** |
| raw / unpacked **integer** reads (e.g. a CF time axis) | **exact** |
| **strings** (CSV/JSON text columns) | **exact** |
| fill / missing (`null` ↔ NaN) | mask must match **element-for-element** |

The float tolerance exists because xarray, NCDatasets, and netcdf-rs differ at the
ULP level when applying `scale_factor`/`add_offset`; everything else is bit-exact.
This is the **only** sanctioned per-language numeric slack, and it is identical to
the oracle [`verify.py`](verify.py).

## Coverage (today)

| case | format | python | julia | rust |
|---|---|---|---|---|
| `era5-grid-sub-tile` | netcdf | ✅ | ✅ | ✅ (three-way) |
| `openaq-points-slice` | csv | ✅ | ✅ | ⊘ no reader |
| `ff10-point-slice` | ff10 | ✅ | ✅ | ✅ (three-way) |
| `isrm-zarr-tile` | zarr | ✅ | ✅ | ✅ (three-way) |
| `permuted-order-tile` | zarr | ✅ | ✅ | ✅ (three-way) |

The Rust track has no `csv` reader yet, so the CSV case is a logged Rust skip
(mirrors [`rust/tests/conformance_decode.rs`](../rust/tests/conformance_decode.rs)),
cross-checked Python↔Julia. Every other case carries the full three-way proof.
When the Rust `csv` reader lands, the dumper reports `csv` in its `readers` and the
coverage gate **automatically requires** it to decode the CSV case — no edit here.

### The `permuted-order-tile` selection gate (Phase 1)

`permuted-order-tile` is a store-backed zarr case that pins **ordered, lazy
orthogonal selection** across all three tracks. Its `select` is
`layer=[0], source=[24,2,9,6] (0-based, NON-CONTIGUOUS and PERMUTED — not sorted),
receptor=all` over an `sr [3,50,4]` array chunked `[1,10,4]`:

* **Laziness** — sources `2,9,6` fall in source-chunk 0 and `24` in chunk 2, so
  only `sr/0.0.0` and `sr/0.2.0` are fetched (never chunk 1, never layers 1/2).
  Proven per-track by [`tests/test_zarr_reader.py`](../tests/test_zarr_reader.py)
  (a `CountingStore` asserting the exact fetched-object set) and
  [`rust/tests/zarr_read_store.rs`](../rust/tests/zarr_read_store.rs) (a poison store).
* **Order preservation** — every track returns the source axis as `[24,2,9,6]` in
  that exact order (a reader that sorted the index list would return `[2,6,9,24]`);
  `crosscheck.py` asserts the three dumps are **byte-identical** here.

The case is data-driven from [`corpus/cases.json`](corpus/cases.json), so
`./run_conformance.sh` runs it in all three tracks and cross-checks it with no
per-case edit — the executable Phase-1 3-way selection acceptance gate.

## Adding a fixture or a reader

The harness is data-driven from [`corpus/cases.json`](corpus/cases.json) and each
track's registered readers — there is nothing per-case to edit:

- **New corpus case** (via [`generate.py`](generate.py)) → every track that has a
  reader for its format decodes it and is cross-checked automatically.
- **New reader in a track** → it appears in that track's dump `readers`, and the
  coverage gate begins requiring it for that format's cases.

---

# Cross-language WRITE conformance (streaming-output-sinks Wave 5)

The harness above proves the three **readers** decode a shared corpus to equal
arrays. This section proves the symmetric claim for the write boundary: the
**Python, Julia, and Rust Zarr v3 sharded WRITERS emit stores that decode to the
same arrays and carry the same structural / CF metadata** — for one shared,
language-neutral input spec.

```
conformance/
  write_spec.json                # the shared input spec (regenerated by gen_write_spec.py)
  gen_write_spec.py              # deterministic regenerator for write_spec.json
  dumpers/write_python.py        # drives earthsciio.backends.zarr_write.ZarrWriter -> store
  dumpers/write_julia.jl         # drives EarthSciIO.ZarrWriter (reference)         -> store
  ../rust/examples/conformance_write.rs  # drives earthsciio::write_zarr_v3         -> store
  dumpers/read_python.py         # reads a store with the Python ZarrReader -> write-native-dump/v1
  dumpers/read_julia.jl          # reads a store with the Julia ZarrReader  -> write-native-dump/v1
  crosscheck_write.py            # asserts: vs spec oracle, pairwise, + store metadata
  run_write_conformance.sh       # driver: run every writer + every reader + crosscheck (the gate)
```

## Run it

```bash
# Python needs >=3.11 with the `zarr` extra (zarr>=3); point PYTHON at it:
PYTHON=/path/to/py311/bin/python conformance/run_write_conformance.sh
conformance/run_write_conformance.sh /tmp/wstores   # keep the stores + dumps
```

Each track whose toolchain is present writes a store; each present reader decodes
every produced store; the comparator gates. A missing toolchain is a logged skip,
never a silent pass.

## The shared input spec — `earthsciio/write-conformance-spec/v1`

`write_spec.json` is a tiny deterministic dataset with the values written out in
full (no language re-derives them from a formula), so it is simultaneously the
**writer input** and the **decode oracle**:

* dims `time` (growable) + `lat`(3) + `lon`(4); `lon` split into **2 inner chunks
  per shard**, `time` sharded **2 records/shard** → 4 records = 2 committed shards
  + a streaming time-axis resize;
* CF coordinate variables `lat`/`lon` (`units`/`standard_name`/`axis`) and a
  growable `time` coordinate with CF time attrs;
* two `float64` variables `temperature`/`pressure` over `(time, lat, lon)` carrying
  CF `units`/`standard_name`/`coordinates`; group attrs `title`/`Conventions`;
* `diagnostic` codec profile (Blosc zstd level 5 + byte-shuffle), sharding codec
  (`sharding_indexed`, inner `[bytes(little), blosc]`, index `[bytes(little),
  crc32c]`, `index_location:"end"`).

Each record's `vars[name]` is a `[lat][lon]` list (row-major); the full variable
array is `[time][lat][lon]`.

## How it works — tolerance, not bytes (RFC §16.6)

3rd-party codec builds (Julia Blosc.jl / Python numcodecs / Rust zarrs) legitimately
produce **different compressed bytes** — verified: the three writers' first
`temperature` shard objects have three distinct sha256s. So conformance is on
**decoded arrays within tolerance** + structural metadata, never byte identity.
Two independent checks in [`crosscheck_write.py`](crosscheck_write.py):

1. **Decoded-array agreement.** Each store is read back to
   `earthsciio/write-native-dump/v1` (one dump per (writer, reader) pair) and
   checked (a) against the spec **oracle** and (b) **pairwise** against every other
   dump — same dtype, dims, dim order, shape, and values within
   `|a-b| <= atol + rtol·|b|` (`atol=1e-9`, `rtol=1e-6` for float64). A fill/`null`
   cell matches only `null`; `fill_value` (0.0) is **not** mapped to NaN.
2. **Structural / CF-metadata agreement.** Each store's `zarr.json` objects are read
   **directly** (language-neutral JSON, no reader) and compared pairwise:
   `data_type`, `shape`, `dimension_names`, the shard grid, the sharding inner
   `chunk_shape`, the Blosc params, `fill_value`, and the key CF attributes
   (`units`/`standard_name`/`axis`/`coordinates`/`calendar`) + group attrs. This
   pins dim order, shape, coord metadata and CF attrs even for a store no local
   reader could open.

The readback drivers run the **real** store-backed readers (the ones the read
harness proves conformant) over a freshly-written local store: `read_python.py`
via a trivial `<base>/<key>`→file cache shim, `read_julia.jl` via a plain online
`Cache` pointed at a `file://` base URL (a local copy, no network).

## Coverage (executed in-sandbox, 2026-07)

| writer store | python reader | julia reader | structural (zarr.json) |
|---|---|---|---|
| python (`ZarrWriter`)       | ✅ | ✅ | ✅ vs julia, vs rust |
| julia (`ZarrWriter`, ref)   | ✅ | ✅ | ✅ vs python, vs rust |
| rust (`write_zarr_v3`)      | ✅ | ✅ | ✅ vs python, vs julia |

All 6 (writer, reader) readbacks agree with the oracle; all 15 pairwise decoded
comparisons agree within tolerance; all 3 stores agree structurally on every
array — a full three-way write-conformance proof. Because Blosc zstd is lossless,
the decoded float64 values are in fact bit-exact across languages (the tolerance is
the sanctioned envelope, not a fudge).

### Environment notes

* **Python** requires Python ≥3.11 + `pip install -e ".[zarr]"` (zarr≥3;
  zarr 3.2.1 known-good). The repo's system Python 3.9 cannot run the writer —
  point `PYTHON` at a 3.11+ interpreter.
* **Rust** builds `conformance_write` against the existing crate deps; it needs the
  sibling `netcdf-reader` path dep present (the same dep the whole crate needs). If
  that fork is absent, `cargo` skips with a logged reason and the gate runs on the
  tracks that built.
* **Julia** uses the minimal `--project=julia` env; the writer/reader load the
  Blosc weakdep extension the same way `dump_julia.jl` does.
