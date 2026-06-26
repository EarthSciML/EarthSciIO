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
```

## Run

```bash
python3 conformance/verify.py     # offline; schema-validate + all cases; exit 1 on failure
python3 conformance/generate.py   # regenerate the corpus (byte-deterministic)
```

`verify.py` is the **executable definition** of conformance; every language
track reproduces its five checks (cache-key agreement, manifest integrity,
reader decode, native-array equality, offline-only). It needs only `numpy` (+
`xarray`/`netCDF4` for the NetCDF case) and optionally `jsonschema` for schema
validation. Other tracks read the **committed** blobs and need no Python.

Blobs are committed directly (≤ a few KB, deterministic). Larger real slices
wait on the git-lfs-vs-`/projects` hosting decision (plan §8).
