# EarthSciIO conformance corpus

Offline golden fixtures + expected native arrays that bind the Python / Julia /
Rust provider tracks to identical behavior. Spec:
[`../spec/conformance.md`](../spec/conformance.md).

```
corpus/
  cache/v1/blobs/<key[:2]>/<key>.<ext>   # golden cached blobs (a real $EARTHSCIDATADIR)
  cache/v1/meta/<key>.json               # manifests
  cases/<id>.json                        # one conformance case per blob
  cases.json                             # case index
generate.py                              # deterministic corpus generator (numpy + netCDF4)
verify.py                                # reference runner / oracle — the 5 conformance checks, offline
dumpers/dump_python.py                   # Python  provider -> native-dump/v1   (cross-language harness)
dumpers/dump_julia.jl                    # Julia   provider -> native-dump/v1
crosscheck.py                            # asserts native-array equality across the 3 tracks
run_conformance.sh                       # driver: run all 3 provider dumpers + crosscheck (CI gate)
CROSSLANG.md                             # the cross-language harness, in full
```

(the Rust dumper lives in the Rust crate: `../rust/examples/conformance_dump.rs`.)

## Run

```bash
python3 conformance/verify.py     # offline; schema-validate + all cases; exit 1 on failure
python3 conformance/generate.py   # regenerate the corpus (byte-deterministic)
./conformance/run_conformance.sh  # offline; run all 3 providers + assert array equality
```

`verify.py` is the **executable definition** of conformance; every language
track reproduces its five checks (cache-key agreement, manifest integrity,
reader decode, native-array equality, offline-only). It needs only `numpy` (+
`xarray`/`netCDF4` for the NetCDF case) and optionally `jsonschema` for schema
validation. Other tracks read the **committed** blobs and need no Python.

## Cross-language harness (`esio-9nb.9`)

`verify.py` proves the Python decode is correct; the **cross-language harness**
([`CROSSLANG.md`](CROSSLANG.md)) proves the Python / Julia / Rust providers decode
the same blob to the **same native arrays**. `run_conformance.sh` runs each
track's provider over the corpus offline, dumps its native arrays
(`earthsciio/native-dump/v1`), and `crosscheck.py` asserts equality against the
oracle and pairwise across tracks — the CI-gated guarantee
([`.github/workflows/conformance.yml`](../.github/workflows/conformance.yml)).

Blobs are committed directly (≤ a few KB, deterministic). Larger real slices
wait on the git-lfs-vs-`/projects` hosting decision (plan §8).
