# The WRITE boundary — the symmetric mirror of the `Reader` side in
# registries.jl (`abstract type Reader end` + `FORMAT_REGISTRY`).
#
# A Writer opens an OUTPUT store (a Zarr v3 group here), appends time records as
# a model integrates, and finalizes consolidated metadata + a per-store output
# manifest. Exactly like the read side, a new output format registers under a
# NEW NAME (`register!(WRITER_REGISTRY, name, impl)`) without touching any
# consumer — the seam is the registry, not an edit.
#
# The three generic functions below are the whole interface; a concrete backend
# subtypes `Writer` and adds methods (see `ZarrWriter` in zarr_write.jl).

"""Open an OUTPUT store and stream time records into it. Keyed by format name in
[`WRITER_REGISTRY`]. The write-path mirror of [`Reader`]."""
abstract type Writer end

# --- interface generic functions (concrete backends add methods) ------------

"""
    write_open!(w, store, base_url, schema; role = :single, n_writers = 1, id = 0,
                region = nothing) -> handle

Open an output store at `base_url` and return an opaque write handle threaded
through the other `write_*` calls. `store` is the output Store (local FS);
`schema` is an [`OutputSchema`].

`role` (spec/writer.md §5): `:single` (one writer, `write_flush!` commits),
`:owner` (writer 0 of `n_writers`: creates the group, coordinates, every array's
`zarr.json` and the manifest, and later commits), or `:contributor` (writer `id`
of `n_writers`: *joins* the store the owner created — reads its manifest and
checks `schema`'s fingerprint — and never writes a shared object). `region` maps
non-time dims to 0-based half-open ranges, `"lat" => (0, 48)`; a region that is
not shard-aligned raises [`RegionNotShardAligned`] before any I/O."""
function write_open! end

"""
    write_record!(w, handle, t, arrays; region = nothing) -> index

Append ONE time record (append mode): `t` is the time coordinate value, `arrays`
maps each variable this writer supplies to its gridded `Array` (the variable's
non-time dims, in order, region-shaped). Returns the record index. A non-`nothing`
`region` must equal the region the handle was opened with."""
function write_record! end

"""
    write_record_at!(w, handle, index, t, arrays; region = nothing)

Write record `index` (0-based) in indexed mode, in any order; `t` must equal the
schema's `times[index]` bitwise. Rewriting a committed record needs
[`write_invalidate!`] first."""
function write_record_at! end

"""
    write_static!(w, handle, name, array)

Write the time-less variable `name` (region-shaped when it is partitioned, full
shape when replicated — owner/single only). Committed by the next commit."""
function write_static! end

"""
    write_blob!(w, handle, scope, key, bytes)

Store a small rank-local payload: `scope` is `:static` or `(:record, i)`. Needs a
v2 store. Committed by the next commit."""
function write_blob! end

"""
    write_flush!(w, handle) -> CommitToken

Write every dirty shard of this writer's region and return the token naming what
was written since the last token. A `:single` writer also commits (when
something is pending)."""
function write_flush! end

"""
    write_commit!(w, handle, tokens) -> manifest

Owner only: verify one token per writer (spec/writer.md §5.3) and commit them;
raises [`CommitError`] (writing nothing) when they do not tile the store."""
function write_commit! end

"""
    write_invalidate!(w, handle, indices)

Owner/single, indexed mode: mark committed records uncommitted so they may be
rewritten (bumps `commit_seq`)."""
function write_invalidate! end

"""
    write_invalidate_statics!(w, handle, names)

Owner/single: mark committed statics uncommitted so they may be rewritten."""
function write_invalidate_statics! end

"""
    write_resume(w, store, base_url; role = :single, n_writers = 1, id = 0,
                 region = nothing, at = nothing, schema = nothing) -> handle

Reopen a store after preemption (spec/writer.md §7); in append mode committed
records with `t > at` are discarded."""
function write_resume end

"""
    write_close!(w, handle, tokens = CommitToken[])

Flush any pending data and finalize: a `:single` writer commits and rewrites the
manifest; the owner commits its own token plus the other writers' `tokens`; a
contributor only checks that everything it wrote has been flushed."""
function write_close! end

# --- the writer registry ----------------------------------------------------

"""Writer registry, keyed by output-format name. The write-path mirror of
[`FORMAT_REGISTRY`]. Populated by `EarthSciIO._register_defaults`; `zarr` is
active, other formats register as `:stub` until implemented."""
const WRITER_REGISTRY = Registry{Any}("writer")

"""A registered-but-unimplemented writer (mirrors [`StubReader`]). Calling it is
a clear error pointing at the wave that will implement it."""
struct StubWriter <: Writer
    name::String
    reason::String
end
for f in (
        :write_open!, :write_record!, :write_record_at!, :write_static!,
        :write_blob!, :write_flush!, :write_commit!, :write_invalidate!,
        :write_invalidate_statics!, :write_resume, :write_close!,
    )
    @eval $f(w::StubWriter, args...; kwargs...) =
        error("output format '", w.name, "' is a registered STUB: ", w.reason)
end
