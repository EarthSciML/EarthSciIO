# EarthSciIO conformance corpus + runner (v1)

**Deliverable (d) of `esio-9nb.1`.** Status: normative.
Corpus: [`../conformance/corpus/`](../conformance/corpus). Reference runner:
[`../conformance/verify.py`](../conformance/verify.py). Generator:
[`../conformance/generate.py`](../conformance/generate.py).

The corpus is the **cross-language correctness guarantee**: the same loader +
the same cached blob must yield the **same native arrays** in Python, Julia, and
Rust — proven **offline**, so FFI is unnecessary (the architecture decision).
The full cross-language harness is `esio-9nb.9`; this spec defines the format it
consumes and ships the Python oracle so the corpus is self-validating today.

---

## 1. The corpus *is* a populated cache

```
conformance/corpus/
  cache/v1/blobs/<key[:2]>/<key>.<ext>    # golden cached blobs (a real $EARTHSCIDATADIR)
  cache/v1/meta/<key>.json                # their manifests
  cases/<id>.json                         # one conformance case per blob (below)
  cases.json                              # case index
```

Point any provider at `conformance/corpus/cache` as `$EARTHSCIDATADIR` with
`offline=true` and every case resolves from disk — no network, no per-language
data tooling beyond the format reader.

### Committed cases (worked examples)

| id | loader | kind | format | transport | store | what it pins |
|---|---|---|---|---|---|---|
| `era5-grid-sub-tile` | era5 | grid | netcdf | file | local | CF scale/offset + `_FillValue`→NaN + a masked cell; packed int16 → float64 |
| `openaq-points-slice` | openaq | points | csv | file | local | a 2nd reader behind the `format` registry; numeric→float64, text→string |
| `ff10-point-slice` | nei2016 | points | ff10 | file | local | FF10 point long-format: `#` header skipped, fixed 77-col schema, RFC-4180 quoted `FACILITY_NAME`, numeric→float64 (blank→NaN), ids/codes→string; 3 rows share one stack (no pivot). member=null decodes the extracted CSV member |

GeoTIFF / Zarr / S3 corpus entries are **format-reserved**: the case + manifest
shape is defined here, but no binary fixture is committed yet — GDAL/git-lfs are
absent in this environment and binary-hosting (git-lfs vs `/projects`) is an
open decision (plan §8). They are added by the GeoTIFF reader / `esio-9nb.8`
work using `generate.py` as the template.

---

## 2. Case format

Each `cases/<id>.json` validates against
[`schemas/cache-case.schema.json`](schemas/cache-case.schema.json) and carries:

- the registry triple (`transport`/`format`/`store`) that reads it;
- `resolved_url` + `cache_key` (with the invariant `sha256(resolved_url) ==
  cache_key`), `blob_path`, `manifest_path`, `content_sha256`, `bytes`;
- optional `select` (which record/rows to slice) and `decode` (conventions hit);
- `expected.variables` — `file_variable` → **native field** (CF-decoded), and
  `expected.coords` — coordinate → native field. Native fields validate against
  [`schemas/native-field.schema.json`](schemas/native-field.schema.json).

---

## 3. <a name="decode"></a>Decode conventions (the parity contract, Risk R4)

Every reader MUST decode identically, or cross-language equality fails. Pinned:

- **CF packing** — apply `scale_factor` / `add_offset`: `value = raw*scale+off`.
  Packed numeric variables are returned as **float64** (the scale/offset math is
  done in double regardless of the on-disk integer width).
- **Fill / missing** — `_FillValue` (and `missing_value` if present) compares
  **before** unpacking; masked elements become **NaN** (encoded as `null` in the
  corpus `data`).
- **Numeric dtype** — unpacked numeric file variables keep an integer logical
  type (`int64`/`int32`); all other numeric reads are **float64**. This removes
  float32-vs-float64 ambiguity between xarray / NCDatasets / netcdf-rs.
- **Time** — the time coordinate is returned **raw** (the stored integer/float
  values) with its `units` + `calendar` carried as metadata. Calendar decoding
  to wall-clock instants is **ESS's** job, not the reader's.
- **Variable identity** — arrays are keyed by the **on-disk `file_variable`**
  name. No remap, no `unit_conversion` (Risk R3 — those stay in ESS).
- **Strings** — text columns (CSV/JSON) are returned as `string` arrays verbatim.

---

## 4. The runner (five checks — identical in every language)

The cross-language harness (`esio-9nb.9`, **shipped** —
[`conformance/CROSSLANG.md`](../conformance/CROSSLANG.md)) drives each track's
**provider** over the corpus offline, dumps its native arrays
(`earthsciio/native-dump/v1`), and asserts equality across Python / Julia / Rust
(and against the oracle). Each track performs exactly what
[`verify.py`](../conformance/verify.py) does:

1. **cache-key agreement** — `sha256(resolved_url) == case.cache_key`.
2. **manifest integrity** — `sha256(blob) == manifest.sha256_content ==
   case.content_sha256` and `len(blob) == manifest.bytes == case.bytes`.
3. **format/reader decode** — open the blob with `case.format`'s reader, applying
   §3.
4. **native-array equality** — decoded arrays/coords equal `case.expected`
   (tolerances below).
5. **offline-only** — the run opens no socket; it reads only corpus files.

### Tolerances

- **Raw / unpacked numeric reads**: compared **exactly**.
- **CF-decoded (packed) values, and any unit-affected reads**: compared within
  `atol = 1e-6`, `rtol = 1e-9` (libraries differ at the ULP level).
- **Strings**: exact. **NaN/fill masks**: must match element-for-element
  (`null` ↔ NaN).

---

## 5. Running it

```bash
python3 conformance/verify.py     # offline; validates schemas + all cases, exit 1 on any failure
python3 conformance/generate.py   # deterministically regenerates the corpus (needs numpy + netCDF4)
./conformance/run_conformance.sh  # offline; run all 3 providers + assert cross-language array equality
```

`generate.py` is committed for provenance and is **byte-deterministic**
(NETCDF3_CLASSIC, fixed data, pinned `fetched_at`) — regenerating does not churn
the committed blobs. Conformance consumers read the **committed** blobs, so no
language track needs Python.

---

## 6. Adding a fixture

1. Add a builder to `generate.py` that returns `(blob_bytes, expected, decode)`
   and an `emit_case(...)` call with the registry triple + a realistic
   `resolved_url`.
2. Run `generate.py` (writes blob + manifest + case + updates `cases.json`).
3. Run `verify.py` — schema validation + the five checks must pass.
4. Keep blobs **tiny** (≤ a few KB) and deterministic; commit directly until the
   binary-hosting decision (git-lfs vs `/projects`) lands for larger real slices.
