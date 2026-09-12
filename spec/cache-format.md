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

0. **Local source recheck** — for a `file://` source, compare the file the URL
   names against the manifest that was written when it was ingested. See
   [§4.1](#41-local-file-sources-are-rechecked-against-their-source); it is a
   separate rung because it is the only one that can consult the source itself.
1. **Content hash** — if a loader-declared checksum exists (none today; future
   `source.checksums` schema field), verify `sha256(blob)` against it. Strongest.
2. **Declared immutability** — a static loader (no `temporal`) or a closed past
   period (e.g. `file_period:P1D` for a past date) cannot change, so it is a hit
   with **no network access at all**. An implementation MUST NOT revalidate one.
3. **Conditional GET** — if `etag`/`last_modified` are stored, revalidate with
   `If-None-Match` / `If-Modified-Since`; `304 Not Modified` ⇒ valid, reuse.
4. **TTL from `temporal`** — a current/incomplete period ⇒ short TTL.

Rules 2 and 3 were previously the other way round, on the reasoning that
validators beat heuristic freshness. They do beat rule 4, which is a heuristic —
but rule 2 is a *declaration*, and a conditional GET against an immutable source
can only ever answer "unchanged". The old order also made rule 2 **unreachable
in practice**: S3 returns an ETag on every object, so every warm cache hit
against an S3-backed store paid a round-trip to learn nothing. Measured on a
596,444-cell zarr store that cost 85.9 ms per chunk against a 0.078 ms local
read, and dominated the wall clock of runs whose data was already on disk.

In **offline mode** (see [offline-mode.md](offline-mode.md)) none of the network
steps run: presence + stored `sha256_content` is the only check. Rung 0 does not
run offline either — offline trades freshness for hermeticity by design
([offline-mode.md §3](offline-mode.md#3-conditional-get--ttl-revalidation-is-suppressed)),
and there is no transport left to re-ingest with.

- **Integrity**: `sha256_content` is always computed and stored on fetch.
  Re-verification on read is cheap and **off by default, on for CI/conformance**.
  Note what it is: it hashes the **cached blob** against its own manifest —
  cache-internal consistency, the copy against the record of the copy. It cannot
  see a replaced source, which is what §4.1 is for; the two are different
  questions and both are worth asking.
- **Invalidation**: bump `v1/` to invalidate everything; delete a single blob on
  hash mismatch; a `cache clear [--loader X] [--before T]` utility (core-track).
  One entry is invalidated by hand by deleting its `meta/<key>.json`: every
  blob has a sibling manifest (§3), and an entry without one is not a valid
  entry, so it re-fetches. `key` is `sha256` of the resolved URL, so
  `printf %s "<resolved url>" | sha256sum` names the file.

---

### 4.1 Local `file://` sources are rechecked against their source

Rules 1-4 all ask whether a **remote** source may have changed, and answer with
validators, a declaration, or a heuristic. A `file://` source needs none of
that: the truth is a `stat` away.

Before serving a warm `file://` entry an implementation **MUST** compare the
named file against the manifest:

| on disk | verdict |
|---|---|
| missing, unreadable, or not a regular file | **re-ingest** (the transport then reports the real absence) |
| length != `bytes` | **re-ingest** |
| length == `bytes` but `sha256(file)` != `sha256_content` | **re-ingest** |
| length == `bytes` and hash matches | **hit** — serve the cached blob, re-ingest nothing |

The hash is required, not optional: a size-only check waves through every
equal-length edit (a float re-encode, a corrected value, a different scenario
year). Nothing new is stored — `bytes` and `sha256_content` are already in every
manifest (§3), so this is **not** a format change and a cache written by an
implementation that predates the rung revalidates correctly.

An entry with **no manifest** has nothing to compare against, so rung 0 abstains
and the implementation's existing manifest-less-entry behaviour stands (Rust and
Python treat it as a miss; Julia serves on presence alone). Forcing a re-ingest
there would not be a freshness check at all — it would break §6, since the blob
is committed before its manifest is written and a racing peer legitimately sees
that state for an instant.

**Why the rung exists.** The key is `sha256(resolved_url)` (§1), so a local file
replaced **in place** keeps its key, and rules 1-4 can only ever call it a hit:
a local file carries no ETag and no `Last-Modified`, and a source with no
`temporal` is *declared* immutable by rule 2. The entry therefore outlives the
file it was made from, silently. Reported in
[EarthSciML/EarthSciAST#293](https://github.com/EarthSciML/EarthSciAST/issues/293):
a 2.8 GB snapshot corpus was replaced at the same paths, every already-read URL
kept returning the old corpus, and a test suite stayed green against a corpus
that no longer existed — including after the data path was pointed at a
directory that does not exist. The failure is directional: an *unresolvable*
source is a hard error, a *stale* one was green.

**Cost.** One extra read of the source, paid only for `file://` entries that
were about to be served — i.e. bounded by the bytes the caller was already about
to read. `EARTHSCI_REVALIDATE_FILE=0` (`0`/`false`/`no`/`off`) turns the rung
off for a corpus known to be immutable; any other value, including an
unparseable one, leaves it **on**.

Every track honours `EARTHSCI_REVALIDATE_FILE` identically, and each also takes
a programmatic override that wins over it:

| track | programmatic opt-out |
|---|---|
| Rust | `Cache::builder().revalidate_file_sources(false)` |
| Python | `Cache(..., revalidate_file=False)` |
| Julia | `Cache(store; revalidate_file = false)` |

> **Track status.** Implemented in **all three tracks** (Rust, Python, Julia),
> which is the point: one track refusing a stale `file://` corpus while another
> serves it would be worse than either behaviour on its own.

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
