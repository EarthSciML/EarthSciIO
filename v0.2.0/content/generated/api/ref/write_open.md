---
title: "write_open"
description: "Stable function · Streaming writer"
type: "api"
api_tabs: true
---


**Tier:** stable · **Kind:** function · **Topic:** [Streaming writer](/generated/api/topics/writer)

| Python | Julia | Rust |
|---|---|---|
| not exported | `write_open!` | not exported |


<div class="api-tabs">

<div class="api-tab" data-lang="python" data-label="Python">
<p class="api-tab-label">Python</p>

<p class="api-absent">Python does not export this function.</p>

</div>

<div class="api-tab" data-lang="julia" data-label="Julia">
<p class="api-tab-label">Julia</p>

```julia
write_open!(w::EarthSciIO.StubWriter, args...; kwargs...)
write_open!(::ZarrWriter, store, base_url::AbstractString, schema::OutputSchema; role, n_writers, id, region)
```

```julia
write_open!(w, store, base_url, schema; role = :single, n_writers = 1, id = 0,
            region = nothing) -> handle
```

Open an output store at `base_url` and return an opaque write handle threaded
through the other `write_*` calls. `store` is the output Store (local FS);
`schema` is an [`OutputSchema`].

`role` (spec/writer.md §5): `:single` (one writer, `write_flush!` commits),
`:owner` (writer 0 of `n_writers`: creates the group, coordinates, every array's
`zarr.json` and the manifest, and later commits), or `:contributor` (writer `id`
of `n_writers`: *joins* the store the owner created — reads its manifest and
checks `schema`'s fingerprint — and never writes a shared object). `region` maps
non-time dims to 0-based half-open ranges, `"lat" => (0, 48)`; a region that is
not shard-aligned raises [`RegionNotShardAligned`] before any I/O.

[Source](https://github.com/EarthSciML/EarthSciIO/blob/a49538bbd6ec318dde369d3496f6e8880586a73f/julia/src/writer.jl#L19)

</div>

<div class="api-tab" data-lang="rust" data-label="Rust">
<p class="api-tab-label">Rust</p>

<p class="api-absent">Rust does not export this function.</p>

</div>

</div>
