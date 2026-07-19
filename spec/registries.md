# EarthSciIO extensibility registries (v1)

**Deliverable (b) of `esio-9nb.1`.** Status: normative.
Machine-readable companion: [`registries.json`](registries.json).

EarthSciIO is **extensible by construction** through three registries. Each
registry is a **name → implementation lookup**. The single load-bearing rule:

> **A new backend registers under a new name WITHOUT touching the Provider API.**
> The Provider depends only on the three *interfaces* below and resolves the
> concrete implementation by name at runtime. Adding S3 transport, a Zarr
> reader, or an object-store backend is a *registration*, never a Provider edit.

This is what lets the future S3-proxy + NetCDF→Zarr cloud path slot in later.
S3 and Zarr are registered **now as stubs** (`status:"stub"` in
`registries.json`) and exercised through the registries by `esio-9nb.8`; their
real implementations land later — with **zero** change to Provider code. What
those real implementations must deliver is charted in
[`cloud-future.md`](cloud-future.md) (the `esio-cloud` epic).

The three registries are orthogonal: a single fetch composes one entry from
each — `transport` gets the bytes, `store` holds them, `format` decodes them.

```
  resolved_url ──▶ [transport:scheme] ──▶ bytes ──▶ [store:name] ──▶ cached blob
                                                          │
                                          cache key = sha256(resolved_url)
                                                          ▼
                                              [format:name] ──▶ native arrays
```

Interfaces are given as **language-neutral pseudo-signatures**. Each language
track binds them to its idiom (Python `Protocol`/ABC, Julia abstract type +
methods, Rust trait); the per-language `Provider` signatures live in the plan
(§4.6) and the per-track beads.

---

## 1. `transport` registry

Keyed by **URL scheme**. Fetches a resolved URL's bytes into the cache.
**Bypassed entirely in offline mode** (the transport is never constructed when
`offline=true`).

```
interface Transport:
    schemes() -> [string]                         # e.g. ["http","https"]
    fetch(resolved_url: string,
          dest: WritablePath,                     # a tmp/<uuid>.part staging path
          conditional: {etag?, last_modified?},   # for revalidation; may be empty
          auth: AuthResolver?) -> FetchResult

FetchResult = {
    status: "downloaded" | "not_modified",        # not_modified ⇒ 304, reuse cache
    etag?: string, last_modified?: string,        # to persist into the manifest
    bytes_written: int
}
```

| name | schemes | status | notes |
|---|---|---|---|
| `http` | `http`, `https` | **active** | GET + conditional GET; mirror failover at the call site |
| `file` | `file` | **active** | local copy; expands `${EARTHSCIDATADIR}` in `file://` templates |
| `cds` | `cds` | **active** | Copernicus CDS API v1: `cds://<dataset>?<request-json>` → submit → poll job → download asset href; auth via the `cds` realm (`PRIVATE-TOKEN`) |
| `s3` | `s3` | **active** | anonymous `s3://<bucket>/<key>` → regional virtual-hosted HTTPS (region default `us-east-2` via `$EARTHSCI_S3_REGION`/`$AWS_REGION`); delegates to the `http` transport (no AWS SDK/SigV4). The `s3://` URL stays canonical in the cache key + manifest |

Registration key = **URL scheme**. The fetch layer reads the resolved URL's
scheme and looks up the transport; an unknown scheme is a registration gap, not
a Provider change. Auth resolvers (CDS/FIRMS/OpenAQ/RDA/bearer) are a separate
pluggable map injected as `auth`, never baked into a transport.

---

## 2. `format` registry

Keyed by **format name** (the "reader" registry). Opens a cached blob and
returns **CF-decoded native-grid arrays keyed by the on-disk `file_variable`
name**, plus native coordinates.

```
interface Reader:
    formats() -> [string]                         # e.g. ["netcdf"]
    extensions() -> [string]                      # sniff hints: ["nc","nc4"]
    open(blob_path: Path) -> Handle
    read_native(handle: Handle,
                variables: [string],              # file_variable names to read
                select: Selection) -> { string: NativeField }   # + coords

NativeField = { dtype, dims: [string], shape: [int], data, fill_value? }
```

| name | ext | status | notes |
|---|---|---|---|
| `netcdf` | `nc`,`nc4`,`cdf` | **active** | CF decode (§decode in [conformance.md](conformance.md#decode)) |
| `geotiff` | `tif`,`tiff` | **active** | raster bands via GDAL; Py first, Jl/Rs may lag (R5) |
| `csv` | `csv` | **active** | points: numeric cols → float64, others → string |
| `json` | `json` | **active** | points (e.g. station-discovery payloads) |
| `zarr` | `zarr` | **active** | **store-backed** Zarr v2: per-array `.zarray`/`.zattrs`, lazy orthogonal chunk selection (fetch only intersecting chunk objects), blosc/lz4+shuffle decode, `<f4`/`<f8`→float64, dims from `_ARRAY_DIMENSIONS`, `fill_value` not→NaN, no coords |

**Hard boundary (Risk R3):** the reader applies **read/decode** semantics only —
CF `scale_factor`/`add_offset`, `_FillValue` → NaN, endianness, chunking. It
does **not** remap `file_variable` → schema name and does **not** apply the
loader's `unit_conversion`. Those are ESS contract semantics and stay in ESS.
The native array is keyed by the **on-disk** variable name.

Format is selected by the loader's declared format (or a content-type /
extension sniff), **never** by trusting the cache-blob suffix alone.

---

## 3. `store` registry

Keyed by **store name** (the "backend" registry). Where the content-addressed
cache physically lives. Realizes the layout in
[cache-format.md](cache-format.md#2-on-disk-layout); the cache **key** is
store-independent.

```
interface Store:
    name() -> string                              # "local"
    exists(key: string) -> bool
    get_blob(key: string) -> Path | bytes | None  # None ⇒ cache miss
    put_blob(key: string, staged: Path) -> void   # atomic commit from tmp staging
    get_meta(key: string) -> Manifest | None
    put_meta(key: string, manifest: Manifest) -> void
    lock(key: string) -> Lock                      # advisory; scope = one blob fetch
```

| name | status | notes |
|---|---|---|
| `local` | **active** | `$EARTHSCIDATADIR` filesystem; `flock` + atomic rename |
| `s3` | **stub** | object store; conditional PUT / `If-None-Match` as the lock analog |

Registration key = **store name** (config-selected). Swapping `local`→`s3`
changes where blobs live; the Provider, the key scheme, and every reader are
untouched.

---

## 4. How the Provider stays unchanged (the invariant, restated)

```
Provider(loader, *, window, offline, auth)        # depends on the 3 INTERFACES only
   transport = transport_registry[scheme_of(url)] # resolved by name
   store     = store_registry[config.store]        # resolved by name
   reader    = format_registry[loader.format]      # resolved by name
```

Adding a backend = add one row to the relevant table in `registries.json` and
register its implementation. No row in this document, and no line in the
Provider, changes shape when S3/Zarr/object-store arrive. `esio-9nb.8` proves
this by registering and exercising the S3 + Zarr **stubs** through exactly these
three lookups.
