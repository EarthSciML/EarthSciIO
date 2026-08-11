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
    write_open!(w, store, base_url, schema) -> handle

Create the group/array metadata (the Zarr v3 `zarr.json` objects), the
dims/coords, and the chunk+shard grid at `base_url`; return an opaque write
handle threaded through [`write_record!`]/[`write_close!`]. `store` is the
output Store (local FS in Wave 1); `schema` is an [`OutputSchema`]."""
function write_open! end

"""
    write_record!(w, handle, t, arrays; region=nothing)

Append ONE time record: `t` is the time coordinate value, `arrays` maps each
streaming variable name to its gridded `Array` (spatial dims only, in the
variable's non-time dim order). A full time-shard is flushed as one
atomically-committed object once its buffer fills. `region` is reserved for
sub-domain writes and MUST be `nothing` in Wave 1."""
function write_record! end

"""
    write_close!(w, handle)

Flush any partial trailing shard, then finalize consolidated metadata and the
output manifest (the crash barrier's durable record)."""
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
write_open!(w::StubWriter, args...; kwargs...) =
    error("output format '$(w.name)' is a registered STUB: $(w.reason)")
write_record!(w::StubWriter, args...; kwargs...) =
    error("output format '$(w.name)' is a registered STUB: $(w.reason)")
write_close!(w::StubWriter, args...; kwargs...) =
    error("output format '$(w.name)' is a registered STUB: $(w.reason)")
