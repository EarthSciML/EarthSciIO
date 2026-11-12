---
title: "dim_window"
description: "Stable type · Prefetch and windows"
type: "api"
api_tabs: true
---


**Tier:** stable · **Kind:** type · **Topic:** [Prefetch and windows](/generated/api/topics/prefetch)

| Python | Julia | Rust |
|---|---|---|
| `DimWindow` | `DimWindow` | `DimWindow` |


<div class="api-tabs">

<div class="api-tab" data-lang="python" data-label="Python">
<p class="api-tab-label">Python</p>

```python
class DimWindow
DimWindow(dims: Mapping[str, Any] | None = None)
@classmethod DimWindow.from_select(select: Mapping[str, Any]) -> DimWindow
DimWindow.dims: dict[str, Any]  # property
DimWindow.select: dict[str, Any]  # property
```

A window that names its dimensions (`spec/prefetch.md` §2.2).

`DimWindow({"lat": (120, 218), "lon": [1151, 0, 1], "lev": "all"})` — a 2-
or 3-tuple is a half-open `slice` `(start, stop[, step])`, a list of ints
is `indices` (kept in the order given, repeats included — a window across a
periodic longitude seam is `[1150, 1151, 0, 1]`), and `"all"` and wire
selector dicts pass through. All indices are **0-based**, the wire
vocabulary. `select` is the wire form `{"dims": {...}}`.

The name is `DimWindow` because [`earthsciio.Window`](/generated/api/ref/window) is already the
run window `(start, end)`. Wherever a `DimWindow` is accepted the wire
dict is accepted too. Instances are immutable; construction validates the
selectors' SHAPE, while bounds are checked against the file at read time.

**Members**

- `from_select` — The window a wire `{"dims": {...}}` select spells.
- `dims` — `{dim: wire selector}` — a copy, in the order given.
- `select` — The wire form, `{"dims": {...}}` — a fresh dict each call.

[Source](https://github.com/EarthSciML/EarthSciIO/blob/a49538bbd6ec318dde369d3496f6e8880586a73f/earthsciio/selection.py#L257)

</div>

<div class="api-tab" data-lang="julia" data-label="Julia">
<p class="api-tab-label">Julia</p>

```julia
struct DimWindow
DimWindow(dims::Vector{Pair{String, Any}})
DimWindow(ps::Pair...)
DimWindow(d::AbstractDict)
```

```julia
DimWindow("lat" => (120, 218), "lon" => [1151, 0, 1], "lev" => "all")
DimWindow(Dict("lat" => (120, 218), ...))
```

A named-dimension window: the Julia constructor for the wire form
`select = {"dims": {...}}` (spec/prefetch.md §2.2). Each pair names a dimension and
a selector, in the WIRE vocabulary — **0-based**, not Julia's:

  * `(start, stop)` — the half-open slice `start, …, stop-1`;
  * `(start, stop, step)` — the same, every `step`-th (`step >= 1`);
  * a `Vector` of integers — those indices, in that order, repeats kept (`[]` is a
    legal empty selection: a zero-length axis);
  * `"all"` (or `:all`) — the whole dimension;
  * a wire selector `Dict("slice" => [...])` / `Dict("indices" => [...])` as is.

A Julia range (`120:217`) is refused rather than guessed at: whether its end is
inclusive and whether it counts from 1 is exactly the off-by-one a window must not
be able to carry silently. Spell the slice `(120, 218)`.

The named form applies each selector to EVERY field and coordinate carrying that
dimension, whatever its rank, and leaves unnamed dimensions whole — so one window
covers a 2-D `ps(time, lat, lon)`, a 3-D `t(time, lev, lat, lon)` and a staggered
`u(time, lat, lon_edge)`. [`select_form`] gives the wire `Dict`; [`with_window`],
[`sample_windows_at`] and `PrefetchGroup(...; windows)` take either. (`Window`
already names the run window `(start, end)` in the Python and Rust tracks, hence
`DimWindow` in every binding.)

[Source](https://github.com/EarthSciML/EarthSciIO/blob/a49538bbd6ec318dde369d3496f6e8880586a73f/julia/src/windows.jl#L11)

</div>

<div class="api-tab" data-lang="rust" data-label="Rust">
<p class="api-tab-label">Rust</p>

```rust
pub struct DimWindow {
    /* private fields */
}

impl DimWindow { pub fn new() -> Self }

impl DimWindow { pub fn range(self, dim: impl Into<String>, start: usize, stop: usize) -> Self }

impl DimWindow { pub fn step_range(self, dim: impl Into<String>, start: usize, stop: usize, step: usize) -> Self }

impl DimWindow { pub fn indices(self, dim: impl Into<String>, indices: impl IntoIterator<Item = usize>) -> Self }

impl DimWindow { pub fn all(self, dim: impl Into<String>) -> Self }

impl DimWindow { pub fn axis(self, dim: impl Into<String>, select: AxisSelect) -> Self }

impl DimWindow { pub fn dims(&self) -> &BTreeMap<String, AxisSelect> }
```

A named-dimension window, built one dimension at a time — the Rust spelling
of `spec/prefetch.md` §2.2's `DimWindow` (the run window `(start, end)`
already owns the name `Window`).

```rust
use earthsciio::{DimWindow, Selection};
// A halo across the periodic longitude seam, the first level only, every
// latitude in [120, 218): one window for 2-D and 3-D fields alike.
let w = DimWindow::new()
    .range("lat", 120, 218)
    .indices("lon", vec![1151, 0, 1])
    .step_range("lev", 0, 1, 1);
let sel: Selection = w.into();
assert!(matches!(sel, Selection::Named(_)));
```

Indices are 0-based, ranges half-open — the wire vocabulary in every binding,
Julia's included. Naming a dimension twice keeps the last selector.

[Source](https://github.com/EarthSciML/EarthSciIO/blob/a49538bbd6ec318dde369d3496f6e8880586a73f/rust/src/format/mod.rs#L493) · [Rust docs](https://docs.rs/earthsciio/latest/earthsciio/format/struct.DimWindow.html)

</div>

</div>
