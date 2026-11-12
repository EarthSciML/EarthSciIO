---
title: "provider"
description: "Stable type · Providers"
type: "api"
api_tabs: true
---


**Tier:** stable · **Kind:** type · **Topic:** [Providers](/generated/api/topics/providers)

| Python | Julia | Rust |
|---|---|---|
| `Provider` | `Provider` | `Provider` |


<div class="api-tabs">

<div class="api-tab" data-lang="python" data-label="Python">
<p class="api-tab-label">Python</p>

```python
class Provider
Provider(loader: DataSource, cache: Cache, window: Window | None = None, *, formats: Registry | None = None, time_origin: _dt.datetime | str | None = None)
Provider.is_const: bool  # property
Provider.coords: dict[str, NativeField]  # property
Provider.materialize(select: Any | None = None) -> NativeDataset
Provider.refresh(t: TimeLike, select: Any | None = None) -> NativeDataset
Provider.sample_at(t: TimeLike) -> NativeDataset
Provider.with_window(w: Any) -> Provider
Provider.sample_windows_at(t: TimeLike, windows: Sequence[Any]) -> list[NativeDataset]
Provider.supports_selection: bool  # property
Provider.store_backed: bool  # property
Provider.array_shape(var: str) -> tuple[int, ...] | None
Provider.refresh_times_closed() -> list[Any]
Provider.refresh_times() -> list[Any]
Provider.window_records() -> list[Any]
Provider.prefetch(window: Window | None = None) -> list[CacheEntry]
```

A loader-bound provider of native-grid arrays, refreshed at the loader's
cadence. See the module docstring for the CONST/DISCRETE contract.

**Parameters**

- `loader` (`DataSource`) — The [`DataSource`](/generated/api/ref/data_source) this provider serves.
- `cache` (`Cache`) — The content-addressed [`Cache`](/generated/api/ref/cache) (component (a)).
- `window` (`Window | None`) — Optional run window `(start, end)` bounding `refresh_times` /
  `prefetch` and priming `materialize` for a DISCRETE loader.
  It is served by the record in effect at its start, every record inside
  it and, for a two-record bracket, the record after it; a window that
  starts before the source's first record or runs past its `end` is
  refused with [`WindowNotCovered`](/generated/api/ref/window_not_covered)
  (spec/esm-sources.md §2.6).
- `formats` (`Registry | None`) — The format registry to resolve the reader from (defaults to the global
  `format_registry`) — the seam that lets a new
  format plug in with no Provider change.
- `time_origin` (`_dt.datetime | str | None`) — Optional instant (a datetime, naive is UTC, or an ISO 8601 string; a
  whole second) that the provider counts time from (spec/esm-sources.md
  §4) — an esm model's time origin. With one, every time the provider
  takes may be a number of seconds from it (a datetime is still read as
  an instant), and every time it returns is one: `refresh_times`,
  `window_records`, a prefetch group's boundaries and anchors, and
  a two-record bracket's time coordinate (`units` `seconds since <origin>`). Without one, a number is Unix seconds and the times it
  returns are datetimes, as before.
- `Raises`, `registered`

**Members**

- `is_const` — True if the bound loader is time-invariant (CONST).
- `coords` — Native coordinates of the current buffer (lat/lon/…); empty until the first `materialize`/`refresh`.
- `materialize` — Materialize the loader's native arrays into the buffer and return them.
- `refresh` — Refresh the buffer to the cadence anchor for time `t` and return it.
- `sample_at` — The dataset `refresh` would return for `t` — data variables AND coordinates — computed without touching any provider buffer (`spec/prefetch.md` §1).
- `with_window` — A NEW provider with the window `w` baked in as its `select`.
- `sample_windows_at` — K windows of `sample_at` `(t)`, from ONE fetch and decode.
- `supports_selection` — True when the bound reader can honour an orthogonal `select` **without materialising the whole array**. A caller uses this to decide whether to push a projection down (`materialize(..., select=…)`) or to read whole and slice on its own side.
- `store_backed` — True when the bound reader reads a directory-like STORE (a Zarr v2 store, each object its own fetch) rather than one blob. Read with `supports_selection` it tells a caller whether a pushed-down `select` shrinks the DOWNLOAD or only the decode.
- `array_shape` — The full native (dims-order) shape of on-disk array `var`, for a honour/refuse pushdown decision, or `None`.
- `refresh_times_closed` — `refresh_times` over the CLOSED window `[lower, upper]`: the same anchors plus the one at the window end itself, when the window end is a cadence anchor before `temporal.end` (spec/esm-sources.md §2.3). The tstops of a solver that integrates over `[t0, tf]` and must refresh at `tf` when `tf` lands on a record. With no window (`upper` is `temporal.end`, where no record is) it equals `refresh_times`.
- `refresh_times` — The cadence anchors at which the data changes and the solver must `refresh` — the solver tstops.
- `window_records` — The anchors of the records that serve the run window, in order (spec/esm-sources.md §2.6): the record in effect at the window start, every record whose anchor is inside the window, the record at the window end itself when there is one (the refresh `refresh_times_closed` adds) and, with `records_per_sample = 2`, the record after the last of these that its bracket reads (unless that is at or past `temporal.end`).
- `prefetch` — Warm the cache for every file the provider will need, WITHOUT decoding.

[Source](https://github.com/EarthSciML/EarthSciIO/blob/770dbc22d557e6e2ae87d9bccd7386f45764e9d0/earthsciio/provider.py#L489)

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

<p class="api-src">From <a href="https://github.com/EarthSciML/EarthSciIO/blob/770dbc22d557e6e2ae87d9bccd7386f45764e9d0/docs/api/examples/python/provider.py#L15-L28">docs/api/examples/python/provider.py</a></p>

</div>

<div class="api-tab" data-lang="julia" data-label="Julia">
<p class="api-tab-label">Julia</p>

```julia
struct Provider
Provider(cache::Cache, format::String, cadence::Cadence, times::Vector{Float64}, url_for::Function, time_dim::Union{Nothing, String}, variables::Union{Nothing, Vector{String}}, reader_kwargs::Dict{Symbol, Any}, source_loader::Union{Nothing, String}, auth_realm::Union{Nothing, String}, records_per_sample::Union{Nothing, Int64}, mirrors_for::Function, plan)
Provider(cache, format, cadence, times, url_for, time_dim, variables, reader_kwargs, source_loader, auth_realm, records_per_sample, mirrors_for, plan)
Provider(cache::Cache, url; format, cadence, times, time_dim, variables, reader_kwargs, source_loader, auth_realm, records_per_sample, mirrors, temporal, window, time_origin)
```

```julia
Provider(cache, url; format, cadence=CONST, times=Float64[], time_dim=nothing,
         variables=nothing, reader_kwargs=NamedTuple(),
         source_loader=nothing, auth_realm=nothing)
```

A data provider over the shared cache.

  * `url` — a resolved-URL `String` (constant source) OR a function `t -> url`
    (a time-varying source whose file/key changes per tick).
  * `format` — reader name in [`FORMAT_REGISTRY`] (`"netcdf"`, `"csv"`, …).
  * `cadence` — [`CONST`] or [`DISCRETE`]. `DISCRETE` requires non-empty `times`;
    `CONST` requires empty `times`.
  * `times` — the discrete cadence grid (sorted on construction); what
    [`refresh_times`] returns. It may span MANY files: pair it with a `t -> url`
    resolver and the cadence walks from one file to the next (see `time_dim`).
  * `time_dim` — when set on a `DISCRETE` provider whose files hold the cadence
    on an internal axis (e.g. a daily file of hourly steps),
    [`materialize`]`(p, t)` slices that dimension to the tick's record.

    The record is located WITHIN its file, not within the whole cadence: tick
    `i` is record `mod1(i, L)` of the file the resolver returns for it, where `L`
    is that file's length along `time_dim`. So a cadence of 56 three-hourly ticks
    over seven 8-record daily files reads record 1 of day 2 at tick 9, not
    record 9 of day 1 (which does not exist). A single file holding the whole
    cadence is the `L >= length(times)` case, where `mod1` is the identity — the
    behaviour is unchanged. This assumes files are UNIFORM in length and that
    their records are consecutive on the cadence; a short/trimmed file mid-run
    shifts every later tick (the Python track avoids that by matching the file's
    own decoded time axis, which the Julia track cannot do because `times` is a
    caller-defined grid rather than the file's raw axis).
  * `records_per_sample` — `nothing` or `1` (default) returns the SINGLE
    at-or-before record with `time_dim` DROPPED (held piecewise-constant); `2`
    returns the TWO bracketing records (floor + successor) with `time_dim`
    RETAINED at length 2 and a canonical 2-element `time_dim` coordinate of Unix
    epoch seconds (seconds from the `time_origin` when given), so a downstream
    model interpolates in time. `2` requires a
    `time_dim`; the successor is read across a file boundary when needed, and at
    the last cadence tick the bracket degenerates to `[last, last]` (equal
    timestamps) so the downstream weight clamps. The provider does pure I/O
    (returns N records) and performs no interpolation itself.
  * `variables` — restrict the returned data variables (coords always kept);
    `nothing` returns all.
  * `reader_kwargs` — extra keywords forwarded to [`read_native`] (e.g. the CSV
    reader's `numeric_columns`).
  * `source_loader` / `auth_realm` — recorded in the cache manifest on fetch.
  * `temporal`, `window` — a [`SourceTemporal`](/generated/api/ref/source_temporal) and a run window
    `(start, stop)` in UTC, as in the Python and Rust tracks (spec/esm-sources.md
    §2.4), in place of `cadence`/`times`/`time_dim`. Times are then Unix
    seconds, and [`refresh_times`] returns the record anchors in the window
    (its end, else the source's `stop`), possibly none; [`window_records`](/generated/api/ref/window_records)
    returns the records that serve it. A window that starts before the source's
    `start`, runs past its `stop` or is empty is refused with
    [`WindowNotCovered`](/generated/api/ref/window_not_covered) (spec/esm-sources.md §2.6).
    A sample at any `t` at or after the source start — inside the window or
    not — reads the record at or before `t` from its own file by the calendar
    (months and years exact), and a two-record bracket's successor is the next
    record unless it is at or past `stop`. `records_per_sample` here, else the
    temporal's, chooses one record or two. `url` is a `String`, a function of
    the file anchor in Unix seconds, or an [`EsmTemplate`](/generated/api/ref/esm_template) expanded there.
  * `time_origin` — with `temporal`, the instant the provider counts time from
    (spec/esm-sources.md §4), usually an esm model's time origin: a `DateTime`
    (UTC) or an ISO 8601 string, a whole second. Every time it takes (`t`, a
    `Real` window bound) and returns ([`refresh_times`], [`window_records`],
    the anchors and boundaries of a [`PrefetchGroup`], a two-record bracket's
    time coordinate, whose `units` become `seconds since <origin>`) is then
    seconds from it. `nothing` (the default) is the Unix epoch, as before. A
    malformed or fractional origin is [`TimeOriginInvalid`](/generated/api/ref/time_origin_invalid). A CONST
    provider has no times and ignores it; one built from `times` refuses it.
  * `mirrors` — fallback URLs tried in order when the primary fetch fails; the
    blob is still cached under the primary URL's key. With an
    [`EsmTemplate`](/generated/api/ref/esm_template) `url`, leave this empty and give the mirrors to the
    template: they are esm templates expanded at the same anchor
    (spec/esm-sources.md §1.5).

[Source](https://github.com/EarthSciML/EarthSciIO/blob/770dbc22d557e6e2ae87d9bccd7386f45764e9d0/julia/src/provider.jl#L25)

**Example** (`provider`)

```julia
url = "file://" * abspath(joinpath(FIXTURES, "stations.csv"))
p = const_provider(
    cache, url; format = "csv", source_loader = "stations",
    variables = ["value", "location_id"],
    reader_kwargs = (numeric_columns = ["latitude", "longitude", "value"],)
)
@assert is_const(p) && refresh_times(p) == Float64[]  # a CONST provider never refreshes
data = materialize(p)
@assert data isa NativeDataset
@assert Set(variable_names(data)) == Set(["location_id", "value"])
@assert data["value"].data == [152.3, 168.7, 98.1, 110.4]
# Digit-only text columns stay strings: nothing is inferred.
@assert data["location_id"].data == ["1", "1", "2", "2"]
```

<p class="api-src">From <a href="https://github.com/EarthSciML/EarthSciIO/blob/770dbc22d557e6e2ae87d9bccd7386f45764e9d0/docs/api/examples/julia/provider.jl#L8-L20">docs/api/examples/julia/provider.jl</a></p>

</div>

<div class="api-tab" data-lang="rust" data-label="Rust">
<p class="api-tab-label">Rust</p>

```rust
pub struct Provider {
    /* private fields */
}

impl Provider { pub fn new(loader: DataSource, cache: Arc<Cache>, window: Option<Window>) -> Result<Self> }

impl Provider { pub fn with_formats(loader: DataSource, cache: Arc<Cache>, window: Option<Window>, formats: &FormatRegistry) -> Result<Self> }

impl Provider { pub fn coords(&self) -> &HashMap<String, Coord> }

impl Provider { pub fn supports_selection(&self) -> bool }

impl Provider { pub fn store_backed(&self) -> bool }

impl Provider { pub fn array_shape(&self, var: &str) -> Result<Option<Vec<usize>>> }

impl Provider { pub fn store_access(&self) -> StoreAccess }

impl Provider { pub fn materialize(&mut self) -> Result<HashMap<String, NativeField>> }

impl Provider { pub fn materialize_with_select(&mut self, sel: Option<&Selection>) -> Result<HashMap<String, NativeField>> }

impl Provider { pub fn refresh(&mut self, t: impl Into<ProviderTime>) -> Result<Option<HashMap<String, NativeField>>> }

impl Provider { pub fn refresh_with_select(&mut self, t: impl Into<ProviderTime>, sel: Option<&Selection>) -> Result<Option<HashMap<String, NativeField>>> }

impl Provider { pub fn sample_at(&self, t: impl Into<ProviderTime>) -> Result<NativeDataset> }

impl Provider { pub fn with_window(&self, w: impl Into<Selection>) -> Result<Provider> }

impl Provider { pub fn sample_windows_at(&self, t: impl Into<ProviderTime>, windows: &[Selection]) -> Result<Vec<NativeDataset>> }

impl Provider { pub fn refresh_times(&self) -> Vec<f64> }

impl Provider { pub fn refresh_times_closed(&self) -> Vec<f64> }

impl Provider { pub fn window_records(&self) -> Vec<f64> }

impl Provider { pub fn time_origin(&self) -> Option<OffsetDateTime> }

impl Provider { pub fn instant(&self, t: impl Into<ProviderTime>) -> Result<OffsetDateTime> }

impl Provider { pub fn run_window(&self, start: impl Into<ProviderTime>, end: impl Into<ProviderTime>) -> Result<Window> }

impl Provider { pub fn prefetch(&mut self, window: Window) -> Result<()> }
```

A loader-bound provider of native-grid arrays, refreshed at the loader's
cadence. See the module-level documentation for the CONST/DISCRETE contract.

[Source](https://github.com/EarthSciML/EarthSciIO/blob/770dbc22d557e6e2ae87d9bccd7386f45764e9d0/rust/src/provider.rs#L634) · [Rust docs](https://docs.rs/earthsciio/latest/earthsciio/struct.Provider.html)

</div>

</div>
