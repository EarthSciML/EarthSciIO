---
title: "take"
description: "Stable function · Prefetch and windows"
type: "api"
api_tabs: true
---


**Tier:** stable · **Kind:** function · **Topic:** [Prefetch and windows](/generated/api/topics/prefetch)

| Python | Julia | Rust |
|---|---|---|
| not exported | `take!` | not exported |


<div class="api-tabs">

<div class="api-tab" data-lang="python" data-label="Python">
<p class="api-tab-label">Python</p>

<p class="api-absent">Python does not export this function.</p>

</div>

<div class="api-tab" data-lang="julia" data-label="Julia">
<p class="api-tab-label">Julia</p>

```julia
take!(m::FileWatching.FolderMonitor)
take!(g::PrefetchGroup, k::Integer)
take!(c::Channel)
take!(io::Base.GenericIOBuffer)
take!(io::IOBuffer)
take!(s::IOStream)
```

```julia
take!(c::Channel)
```

Removes and returns a value from a `Channel` in order. Blocks until data is available.
For unbuffered channels, blocks until a `put!` is performed by a different task.

**Examples**

**Buffered channel**
```julia
julia> c = Channel(1);

julia> put!(c, 1);

julia> take!(c)
1
```

**Unbuffered channel**
```julia
julia> c = Channel(0);

julia> task = Task(() -> put!(c, 1));

julia> schedule(task);

julia> take!(c)
1
```

---

```julia
take!(b::IOBuffer)
```

Obtain the contents of an `IOBuffer` as an array. Afterwards, the `IOBuffer` is reset to its initial state.

**Examples**
```julia
julia> io = IOBuffer();

julia> write(io, "JuliaLang is a GitHub organization.", " It has many members.")
56

julia> String(take!(io))
"JuliaLang is a GitHub organization. It has many members."
```

---

```julia
take!(g::PrefetchGroup, k::Integer) -> EpochSample
```

Block until epoch `k` is decoded and return it (spec/prefetch.md §3.3–3.4). `k`
must be the next entry of the order declared by [`start!`]; anything else is an
error naming the epoch expected, and consumes nothing. Taking after the order is
exhausted, before any `start!`, after [`cancel!`], or after a `take!` raised a
stored error is an error too: call `start!` again to re-seek.

A decode (or `transform`) error is raised by the `take!` of the slot it belongs to
and never earlier — if several members fail, the first by key order — exactly as
`refresh` would raise it (no wrapper).

[Source](https://github.com/EarthSciML/EarthSciIO/blob/a49538bbd6ec318dde369d3496f6e8880586a73f/julia/src/prefetch.jl#L395)

</div>

<div class="api-tab" data-lang="rust" data-label="Rust">
<p class="api-tab-label">Rust</p>

<p class="api-absent">Rust does not export this function.</p>

</div>

</div>
