---
title: "data_source"
description: "Stable type · Providers"
type: "api"
api_tabs: true
---


**Tier:** stable · **Kind:** type · **Topic:** [Providers](/generated/api/topics/providers)

| Python | Julia | Rust |
|---|---|---|
| `DataSource` | not exported | `DataSource` |


<div class="api-tabs">

<div class="api-tab" data-lang="python" data-label="Python">
<p class="api-tab-label">Python</p>

```python
class DataSource
DataSource(name: str, format: str, url: UrlSpec, variables: Sequence[str] = (), temporal: SourceTemporal | None = None, mirrors: Sequence[str] = (), auth_realm: str | None = None, reader_kwargs: dict[str, object] = dict())
DataSource.is_const: bool  # property
DataSource.resolve_url(anchor: _dt.datetime) -> str
```

The I/O-relevant projection of an ESS `DataSource` the Provider needs.

The full ESM contract (units, variable remap, grid family) lives upstream in
ESS; this is only what resolves bytes and decodes them.

**Parameters**

- `name` (`str`) — Loader name — provenance, recorded in the cache manifest.
- `format` (`str`) — Format-registry key selecting the reader (e.g. `"netcdf"`, `"csv"`).
- `url` (`UrlSpec`) — A literal URL (CONST), a `strftime` template resolved at each file
  anchor (e.g. `".../era5/%Y/%m/%Y%m%d.nc"`), or a callable
  `anchor -> url`.
- `variables` (`Sequence[str]`) — On-disk `file_variable` names to read; empty ⇒ all data variables
  (coordinates are always kept).
- `temporal` (`SourceTemporal | None`) — The cadence (`None` ⇒ CONST/static).
- `mirrors` (`Sequence[str]`) — Failover URL candidates sharing the same cache identity.
- `auth_realm` (`str | None`) — Auth realm to fetch under (resolved by the cache's auth registry).
- `reader_kwargs` (`dict[str, object]`) — Extra keywords forwarded to the reader's `read_native` (e.g. the CSV
  reader's `numeric_columns`).

**Members**

- `is_const` — True if the loader is time-invariant (no [`SourceTemporal`](/generated/api/ref/source_temporal)).
- `resolve_url` — Resolve the file URL for a cadence/file anchor.

[Source](https://github.com/EarthSciML/EarthSciIO/blob/a49538bbd6ec318dde369d3496f6e8880586a73f/earthsciio/provider.py#L126)

**Example** (`provider`)

```python
source = DataSource(
    "stations",
    "csv",  # the format-registry key that selects the reader
    (FIXTURES / "stations.csv").as_uri(),
    variables=["value", "location_id"],
    reader_kwargs={"numeric_columns": ["latitude", "longitude", "value"]},
)
data = Provider(source, cache).materialize()
assert isinstance(data, NativeDataset)
assert data.variable_names() == ["location_id", "value"]
assert data["value"].data.dtype == np.float64
assert list(data["value"].data) == [152.3, 168.7, 98.1, 110.4]
# Digit-only text columns stay strings: nothing is inferred.
assert data["location_id"].data == ["1", "1", "2", "2"]
```

<p class="api-src">From <a href="https://github.com/EarthSciML/EarthSciIO/blob/a49538bbd6ec318dde369d3496f6e8880586a73f/docs/api/examples/python/provider.py#L15-L28">docs/api/examples/python/provider.py</a></p>

</div>

<div class="api-tab" data-lang="julia" data-label="Julia">
<p class="api-tab-label">Julia</p>

<p class="api-absent">Julia does not export this type.</p>

</div>

<div class="api-tab" data-lang="rust" data-label="Rust">
<p class="api-tab-label">Rust</p>

```rust
pub struct DataSource {
    pub name: String,
    pub format: String,
    pub variables: Vec<String>,
    pub temporal: Option<SourceTemporal>,
    pub url_template: String,
    pub mirrors: Vec<String>,
    pub auth_realm: Option<String>,
    pub select: Selection,
    pub reader_options: Map<String, Value>,
    pub store_access: Option<StoreAccess>,
    pub store_options: Vec<(String, String)>,
}

impl DataSource { pub fn new(name: impl Into<String>, format: impl Into<String>, url_template: impl Into<String>) -> Self }

impl DataSource { pub fn select(self, select: Selection) -> Self }

impl DataSource { pub fn reader_options(self, options: Map<String, Value>) -> Self }

impl DataSource { pub fn store_access(self, access: StoreAccess) -> Self }

impl DataSource { pub fn store_options<I, K, V>(self, options: I) -> Self where I: IntoIterator<Item = (K, V)>, K: Into<String>, V: Into<String> }

impl DataSource { pub fn variables<I, S>(self, vars: I) -> Self where I: IntoIterator<Item = S>, S: Into<String> }

impl DataSource { pub fn temporal(self, temporal: SourceTemporal) -> Self }

impl DataSource { pub fn mirrors<I, S>(self, mirrors: I) -> Self where I: IntoIterator<Item = S>, S: Into<String> }

impl DataSource { pub fn auth_realm(self, realm: impl Into<String>) -> Self }
```

The minimal parsed `DataSource` the Provider needs: where the bytes are, how
to decode them, which variables to read, and the temporal cadence (absent ⇒
CONST). This is the I/O-relevant projection of the ESM `DataSource` contract —
the full contract (units, remap, grid family) lives upstream in ESS.

[Source](https://github.com/EarthSciML/EarthSciIO/blob/a49538bbd6ec318dde369d3496f6e8880586a73f/rust/src/provider.rs#L194) · [Rust docs](https://docs.rs/earthsciio/latest/earthsciio/struct.DataSource.html)

</div>

</div>
