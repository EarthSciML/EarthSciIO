# EarthSciIO shared cache + manifest format (v1)

**Deliverable (a) of `esio-9nb.1`.** Status: normative. Cache-format version: `v1`.

This is the contract that makes the cache **shared across languages**: a file
fetched by the Python track must be reused, byte-for-byte and without re-fetch,
by the Julia and Rust tracks (and vice-versa). It is realized by the
[`store` registry](registries.md#3-store-registry); the `local`
backend's on-disk form is specified here.

---

## 1. Cache key

The cache is **content-addressed by the resolved URL**: `key =
sha256(resolved_url)`.

```
key = lowercase_hex( sha256( utf8(resolved_url) ) )
```

- `resolved_url` is the URL **after** time-anchor + parameter expansion — i.e.
  the exact URL that would be fetched for one time slice / tile. Per-time-slice
  files are therefore distinct cache entries. (URL resolution itself — the port
  of ESS `time_resolution` + `url_template` — is pure and lives in the cores;
  this spec starts from the already-resolved URL.)
- Encoding is **UTF-8, no trailing newline**, the URL exactly as resolved (no
  normalization, no percent-encoding changes, no case folding). All three
  languages MUST hash the identical byte string.
- Sub-range / byte-slice requests append `#bytes=<a>-<b>` to the URL **before**
  hashing, so a sub-slice is its own entry.

The key is **store-independent**: the same key addresses the blob whether it
lives on local disk or in an S3 object store.

> Worked example (from the conformance corpus):
> `sha256("https://data.earthsci.dev/era5/2018/11/20181108.nc")` =
> `11cdcec111409f586e6afc432e1a6da47e6f97ccf3715e5db8554632b00671c1`.

---

## 2. On-disk layout

The `local` store backend's form. Root = `$EARTHSCIDATADIR` (see §5). Everything lives under a version directory
so a format bump invalidates the whole cache by changing one path segment.

```
$EARTHSCIDATADIR/
  v1/                                       # cache-format version (this spec)
    blobs/<key[:2]>/<key>.<ext>             # the downloaded file
    meta/<key>.json                         # the manifest (§3)
    locks/<key>.lock                        # per-blob advisory lock (§6)
    tmp/<uuid>.part                         # atomic-rename staging (§6)
```

- `<key[:2]>` is the first two hex chars of the key — a 256-way fan-out so no
  single directory holds every blob.
- `<ext>` is taken from the URL / content-type for **human debuggability only**.
  Lookups are by `<key>`, never by extension; a reader is selected by the
  [`format` registry](registries.md#2-format-registry), not the suffix.
- The filesystem **is** the index — there is no separate index database. A
  loader-level manifest (all anchors covering a run window) is computed on
  demand from URL resolution, not stored.

---

## 3. Manifest — `meta/<key>.json`

Every blob has a sibling manifest carrying its validation + provenance state.
Schema: [`schemas/manifest.schema.json`](schemas/manifest.schema.json).

| Field | Type | Required | Meaning |
|---|---|---|---|
| `schema` | `"earthsciio/manifest/v1"` | no | manifest schema tag |
| `url` | string | **yes** | the resolved source URL (its sha256 is `key`) |
| `etag` | string \| null | yes¹ | HTTP ETag, for conditional GET (`If-None-Match`) |
| `last_modified` | string \| null | yes¹ | HTTP Last-Modified, for `If-Modified-Since` |
| `sha256_content` | hex(64) | **yes** | sha256 of the blob bytes (self-pinned integrity) |
| `bytes` | int ≥ 0 | **yes** | blob size; MUST equal the on-disk length |
| `fetched_at` | RFC 3339 UTC | **yes** | when the blob was fetched |
| `source_loader` | string \| null | yes¹ | `.esm` loader that resolved the URL (provenance) |
| `auth_realm` | string \| null | yes¹ | realm used (e.g. `cds`), or null; **never credentials** |

¹ The key carries `null` when not applicable; the field is always present.

The manifest maps the bead's required fields directly:
**source-url** → `url`, **etag** → `etag`, **checksum** → `sha256_content`,
**fetched-at** → `fetched_at`, **byte-size** → `bytes`.

Credentials are **never** written to the manifest (only the realm name). The
`.esm` contract carries no cache/auth/checksum fields — those are runtime-only
and owned here, consistent with the ESS schema's stated intent.

---

## 4. Validation and integrity

A cache **hit** requires the blob to be present **and valid**. Validity is
decided in this order (first applicable wins):

1. **Content hash** — if a loader-declared checksum exists (none today; future
   `source.checksums` schema field), verify `sha256(blob)` against it. Strongest.
2. **Conditional GET** — if `etag`/`last_modified` are stored, revalidate with
   `If-None-Match` / `If-Modified-Since`; `304 Not Modified` ⇒ valid, reuse.
3. **TTL from `temporal`** — a closed past period (e.g. `file_period:P1D` for a
   past date) is immutable ⇒ infinite TTL; a current/incomplete period ⇒ short
   TTL. Static loaders (no `temporal`) are immutable once fetched.

In **offline mode** (see [offline-mode.md](offline-mode.md)) none of the network
steps run: presence + stored `sha256_content` is the only check.

- **Integrity**: `sha256_content` is always computed and stored on fetch.
  Re-verification on read is cheap and **off by default, on for CI/conformance**.
- **Invalidation**: bump `v1/` to invalidate everything; delete a single blob on
  hash mismatch; a `cache clear [--loader X] [--before T]` utility (core-track).

---

## 5. `$EARTHSCIDATADIR` resolution

```
dir = env EARTHSCIDATADIR
      || default( /scratch.local/$USER/earthsci-cache )
```

- The environment variable **always wins**; the default is only the fallback.
- The default lives on `/scratch.local`, **never `/u`** — the home inode quota
  cannot absorb many small NetCDF slices (Risk R6). This is a hard rule.
- The provider also expands `${EARTHSCIDATADIR}` inside `file://` mirror
  templates (the `nei2016` pattern) so a pre-populated local mirror is found.

---

## 6. Concurrency — locking + atomic rename

Multiple polecats/processes share one `/scratch.local` cache, so a fetch is:

1. Compute `key`; if `blobs/<key[:2]>/<key>` is present **and** valid → return it
   (a hit takes **no lock**).
2. Otherwise acquire the per-blob advisory lock (`flock` on `locks/<key>.lock`;
   Julia `mkpidlock`, Rust `fs2`, Python `fcntl`/`filelock`), **re-check**
   (another process may have just filled it), download to `tmp/<uuid>.part`,
   verify, **atomically rename** into `blobs/`, then write `meta/<key>.json`.

The atomic rename is the real guarantee — a reader never sees a partial file
even without taking the lock. The advisory lock merely prevents redundant
concurrent downloads. A Julia process and a Python process racing the same URL
is therefore safe and results in exactly one download.

---

## 7. What this format deliberately does **not** hold

- No regridded / reprojected arrays — the cache stores **native** source bytes
  only; regrid is ESD/C4's job.
- No variable-name remap or unit conversion — readers return raw `file_variable`
  arrays (see [conformance.md](conformance.md#decode)); remap stays in ESS.
- No credentials — only the `auth_realm` name.
- No solver state — EarthSciIO provides data, not a solve.
