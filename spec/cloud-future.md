# EarthSciIO cloud path — the `esio-cloud` future epic (stub charter)

**Companion to [`registries.md`](registries.md).** Status: informative (a
charter, not a contract). Tracking epic: **`esio-cloud`** (not yet filed).

The three registries ship **stubs** for the cloud backends *now*
(`esio-9nb.8`), registered under their real names with `status:"stub"` in
[`registries.json`](registries.json) and exercised by
[`../tests/test_registry_dispatch.py`](../tests/test_registry_dispatch.py). They
resolve by name and are interface-conformant; every operation raises
`earthsciio.Unsupported`. This document enumerates what the **real**
implementations must deliver when `esio-cloud` lands.

The load-bearing constraint, restated: **the real backends replace the stub
bodies (or register active implementations under the same names) with ZERO
change to the Provider API, the cache key scheme, or the language cores.** The
only edit to this spec is flipping the relevant `status` from `"stub"` to
`"active"` in `registries.json`. If a real S3/Zarr implementation forces a
Provider signature change, the seam design (`registries.md` §4) has failed and
the change must be reconsidered — not the Provider.

---

## 1. `s3` transport (registry: `transport`, scheme `s3`)

**LANDED (wave2-zarr-s3):** `status:"active"` in all three tracks. The anonymous
read path is implemented as an `s3://<bucket>/<key>` → regional virtual-hosted
HTTPS rewriter delegating to the `http` transport (region default `us-east-2`
via `$EARTHSCI_S3_REGION`/`$AWS_REGION`); the `s3://` URL stays canonical in the
cache key + manifest. **Still `esio-cloud` scope:** SigV4 / requester-pays auth
(the `auth` resolver seam is threaded through unchanged so it plugs in with no
transport edit) and a bucket→region probe. The original charter below stands for
those:

- **Object GET** for `s3://bucket/key` resolved URLs, returning bytes into the
  cache staging path (`tmp/<uuid>.part`) exactly as the `http` transport does;
  the `store` then atomically commits (`cache-format.md` §6).
- **Auth seam reuse** — anonymous (public buckets), AWS SigV4 (env / profile /
  instance role), and requester-pays, via the same pluggable `auth` resolver the
  `http` transport takes (`registries.md` §1) — **never** baked into the
  transport. S3-compatible endpoints (MinIO, Ceph, Wasabi) via an endpoint/region
  config.
- **Conditional GET** — persist + send `ETag` (`If-None-Match`) and
  `Last-Modified` (`If-Modified-Since`); map a not-modified response to
  `FetchResult.status = "not_modified"` so the cache is reused
  (`cache-format.md` §4).
- **Byte-range requests** — honor the `#bytes=a-b` sub-slice convention
  (`cache-format.md` §1) via the HTTP `Range` header, so a sub-slice is its own
  cache entry.
- **Resilience** — retry with backoff on throttling/5xx; participate in
  call-site **mirror failover** (try next mirror on error), identical to `http`.

## 2. `s3` store (registry: `store`, name `s3`)

The future object-store home for the content-addressed cache. Today every
`S3Store` operation → `Unsupported`. Real implementation must:

- **Realize the cache layout as objects** — `blobs/<key[:2]>/<key>.<ext>`,
  `meta/<key>.json` as object keys (`cache-format.md` §2). The cache **key**
  (`sha256(resolved_url)`) is store-independent, so a blob cached by the `local`
  store is addressable here unchanged — that is what makes the cache portable
  to the cloud.
- **`exists` / `get_blob` / `get_meta`** via `HEAD` / `GET`; a miss returns
  `None` (never an error).
- **`put_blob` / `put_meta`** via `PUT` (multipart for large blobs); integrity =
  the stored `sha256_content` (`cache-format.md` §3, §4).
- **Locking analog** — object stores have no `flock`. Use **conditional PUT /
  `If-None-Match: *`** (or a compare-and-swap marker object) as the
  `lock(key)` analog so concurrent writers don't both upload
  (`registries.md` §3). The atomic-rename guarantee becomes "a blob object
  appears whole or not at all" (single PUT / completed multipart).

## 3. `zarr` reader (registry: `format`, format `zarr`)

**LANDED (wave2-zarr-s3):** `status:"active"` in all three tracks as a
**store-backed** reader (the Provider hands it `(cache, base_url, variables,
select)` — a default-off, additive capability; active whole-file readers are
untouched). It parses per-array `.zarray`/`.zattrs` (Zarr v2; **no** consolidated
`.zmetadata`), computes the chunk set a lazy **orthogonal** selection intersects
(`select = {axes: [...]}`), and fetches **only** those chunk objects (each its
own `sha256(object_url)` blob) through the existing cache — blosc/lz4+shuffle
decode, C-order unpack, `<f4`/`<f8`→float64, dims from `_ARRAY_DIMENSIONS`, no
coords. **Deliberate deviation:** `fill_value` is **not** mapped to NaN (0.0 is
real ISRM data). Proven by the `isrm-zarr-tile` conformance case (three-way
byte/value equality). **Still `esio-cloud` scope:** Zarr **v3** + sharding, the
NetCDF→Zarr conversion, non-`lz4` codecs, and `order:"F"`/filter pipelines
(defended against but unexercised). The original charter below stands for those:

- **Open a chunked store** — local directory *and* remote (`s3://…`) via the
  same `store`/transport seam; read **consolidated metadata** when present.
- **Native-array reads** — return arrays keyed by the on-disk `file_variable`
  name with native coords, honoring `select` for **lazy / region** reads aligned
  to the native grid (the whole point of Zarr: don't materialize the globe to
  read a tile).
- **Decode parity (Risk R4)** — CF-decode with conventions **byte-identical** to
  the `netcdf` reader (`conformance.md` §3): `scale_factor`/`add_offset`,
  `_FillValue`→NaN, integer-vs-float64 logical types, time returned **raw** with
  `units`/`calendar`. Cross-language array equality depends on this.
- **No remap / no unit-conversion** (Risk R3) — same hard boundary as every
  reader: those stay in ESS.
- **`zarr` v2 and v3** layouts; the NetCDF→Zarr conversion that produces the
  store is part of this epic (it is the "cloud-native chunked store for
  S3-hosted runs" the locked decision names).

---

## 4. Cross-cutting requirements for `esio-cloud`

- **Conformance fixtures** — add the `s3`/`zarr` corpus entries that
  `conformance.md` §1 reserves ("format-reserved": case + manifest shape defined,
  no binary committed yet). A Zarr fixture + an `s3`-store case let the
  five-check runner (`conformance.md` §4) cover the cloud path offline. The
  binary-hosting decision (git-lfs vs `/projects`, plan §8) gates committing real
  slices.
- **Live smoke tests** — one network-gated (`EARTHSCI_LIVE=1`) test per cloud
  realm, **never** in CI (`offline-mode.md` §4); cached results become fixtures.
- **Cross-language parity** — Julia and Rust implement the same `s3`/`zarr`
  backends against this charter so a blob fetched/decoded by one language is
  reused by the others (`offline-mode.md` §5, `cache-format.md` §1).
- **Status flip only** — landing each backend flips its `registries.json`
  `status` from `"stub"` to `"active"`; no new registry, no Provider edit, no
  cache-key change.

## 5. What is explicitly **out of scope** for the stubs (this bead)

`esio-9nb.8` ships *only* the registered stubs + the dispatch tests + this
charter. No network code, no boto3/fsspec dependency, no NetCDF→Zarr conversion,
no S3 corpus fixtures. Those are `esio-cloud`. The stubs exist to **prove the
seam holds** so that epic is a drop-in.
