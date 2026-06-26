# EarthSciIO provider spec (v1)

The language-neutral contract the Python / Julia / Rust provider tracks
implement against. This spec **gates the cores** (`esio-9nb.{2,4,6}`) and the
providers (`esio-9nb.{3,5,7}`); the cross-language conformance harness
(`esio-9nb.9`) and the S3/Zarr extensibility stubs (`esio-9nb.8`) consume it.

Cache-format version: **`v1`**. Bumping `v1/` invalidates every cached blob.

---

## What EarthSciIO is (the boundary)

EarthSciIO is the **impure provider** between *"a parsed `DataLoader` contract +
a target time/grid"* and *"native-grid arrays in memory"*. It owns: URL
resolution at a time, download + content-addressed cache, and read → native
arrays. It does **not** own the contract (ESM), the schema/types (ESS), regrid /
reproject (ESD), or the solver (user). It returns **raw native-grid arrays** —
variable-remap and regrid stay upstream/downstream.

Architecture (locked, epic `esio-9nb`): **idiomatic per-language implementations
against this shared contract**, with a **shared content-addressed cache spec**
so a file one language fetches is reused by the others — *not* a Rust+FFI core.
Extensibility is by construction via the **three registries** below.

---

## The deliverables of this spec

| # | Deliverable | Doc |
|---|---|---|
| (a) | Shared cache + manifest format; `key = sha256(resolved_url)` | [cache-format.md](cache-format.md) |
| (b) | The 3 extensibility registries (transport / format / store) | [registries.md](registries.md) · [registries.json](registries.json) |
| (c) | Offline-mode contract (cache-only, hermetic CI) | [offline-mode.md](offline-mode.md) |
| (d) | Conformance corpus (offline golden fixtures + expected arrays) | [conformance.md](conformance.md) · [../conformance/](../conformance) |

JSON Schemas: [schemas/manifest.schema.json](schemas/manifest.schema.json),
[schemas/cache-case.schema.json](schemas/cache-case.schema.json),
[schemas/native-field.schema.json](schemas/native-field.schema.json).

---

## The Provider surface (context; detailed impl is the per-track beads)

This spec owns the cache/manifest format, the three registries, offline mode,
and the conformance corpus. The `Provider` object that sits **on top** of them
is specified per-language in the plan (§4.6); its surface is summarized here so
the registries have a consumer to point at. The invariant this spec guarantees:
**the Provider depends only on the three registry interfaces** — new backends
register without touching it.

```
Provider(loader, *, window, offline=false, auth=None)   # bound to one loader
    materialize()      -> { file_variable: NativeField }       # CONST: call once
    refresh(t)         -> { file_variable: NativeField } | None # DISCRETE: at a cadence tstop; None if unchanged
    refresh_times()    -> [float]                              # the data_ingest schedule (solver tstops)
    prefetch(window)   -> None                                 # warm the cache offline-ready

# component (a) seam drop-in (Phase-1 complement of ESS load_data):
cached_opener(loader, *, cache=None, offline=false, auth=None)  -> (url -> Dataset)
cached_fetcher(loader, *, cache=None, offline=false, auth=None) -> (url -> bytes)
```

`NativeField = { array, coords }` on the loader's **native** grid (see
[native-field.schema.json](schemas/native-field.schema.json)). `CONST` vs
`DISCRETE` follows the loader's `temporal` presence. Regrid is ESD/C4's job.

---

## Verifying this spec

```bash
python3 conformance/verify.py     # offline: schema-validate + run all golden cases
python3 conformance/generate.py   # deterministically (re)build the corpus
```
