---
title: "write_close"
description: "Stable function · Streaming writer"
type: "api"
api_tabs: true
---


**Tier:** stable · **Kind:** function · **Topic:** [Streaming writer](/generated/api/topics/writer)

| Python | Julia | Rust |
|---|---|---|
| not exported | `write_close!` | not exported |


<div class="api-tabs">

<div class="api-tab" data-lang="python" data-label="Python">
<p class="api-tab-label">Python</p>

<p class="api-absent">Python does not export this function.</p>

</div>

<div class="api-tab" data-lang="julia" data-label="Julia">
<p class="api-tab-label">Julia</p>

```julia
write_close!(w::EarthSciIO.StubWriter, args...; kwargs...)
write_close!(w::ZarrWriter, h::EarthSciIO.ZarrWriteHandle)
write_close!(w::ZarrWriter, h::EarthSciIO.ZarrWriteHandle, tokens::AbstractVector)
```

```julia
write_close!(w, handle, tokens = CommitToken[])
```

Flush any pending data and finalize: a `:single` writer commits and rewrites the
manifest; the owner commits its own token plus the other writers' `tokens`; a
contributor only checks that everything it wrote has been flushed.

---

```julia
write_close!(w, h, tokens = CommitToken[])
```

`:single`: flush (committing pending data), rewrite the group node and the
manifest at the same `commit_seq`; returns the manifest (an [`OutputManifest`]
for a v1 store, the parsed v2 object otherwise). `:owner`: flush its own data,
commit `[own token; tokens]` (the OTHER writers' tokens) and return the manifest.
`:contributor`: takes no tokens, errors if anything written has not been flushed,
returns `nothing`.

[Source](https://github.com/EarthSciML/EarthSciIO/blob/a49538bbd6ec318dde369d3496f6e8880586a73f/julia/src/writer.jl#L103)

</div>

<div class="api-tab" data-lang="rust" data-label="Rust">
<p class="api-tab-label">Rust</p>

<p class="api-absent">Rust does not export this function.</p>

</div>

</div>
