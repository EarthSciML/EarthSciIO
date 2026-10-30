---
title: "materialize"
description: "Stable function · Providers"
type: "api"
api_tabs: true
---


**Tier:** stable · **Kind:** function · **Topic:** [Providers](/generated/api/topics/providers)

| Python | Julia | Rust |
|---|---|---|
| not exported | `materialize` | not exported |


<div class="api-tabs">

<div class="api-tab" data-lang="python" data-label="Python">
<p class="api-tab-label">Python</p>

<p class="api-absent">Python does not export this function.</p>

</div>

<div class="api-tab" data-lang="julia" data-label="Julia">
<p class="api-tab-label">Julia</p>

```julia
materialize(p::Provider, t::Real; select)
materialize(p::Provider; select)
```

```julia
materialize(p::Provider, t::Real; select=nothing) -> NativeDataset
materialize(p::Provider; select=nothing) -> NativeDataset
```

Return the native arrays for the source at time `t`. For a [`DISCRETE`] provider
with `time_dim`, the internal cadence axis is sliced to `t`'s record — unless
`records_per_sample=2`, in which case the two bracketing records (floor +
successor) are returned with `time_dim` retained at length 2 and a 2-element
epoch-seconds `time_dim` coordinate (see the [`Provider`] `records_per_sample`
field). The no-argument form is for a [`CONST`] provider (a `DISCRETE` provider
must be given a time).

`select` is an optional PER-CALL projection pushdown (in EarthSciIO's native
select shape, e.g. `Dict("axes"=>[...])` with 0-based indices). When supplied it
OVERRIDES any baked `reader_kwargs[:select]` for this call only, letting a caller
(EarthSciAST) push a projection down at sample time without rebuilding the
provider. Any reader that answers [`supports_selection`] can honour it — the
`zarr` reader by fetching only the intersecting chunk objects, the `netcdf` one by
materialising only the requested hyperslab of the blob it already fetched;
passing `select` to a reader that cannot is an `ArgumentError`.

[Source](https://github.com/EarthSciML/EarthSciIO/blob/a49538bbd6ec318dde369d3496f6e8880586a73f/julia/src/provider.jl#L602)

</div>

<div class="api-tab" data-lang="rust" data-label="Rust">
<p class="api-tab-label">Rust</p>

<p class="api-absent">Rust does not export this function.</p>

</div>

</div>
