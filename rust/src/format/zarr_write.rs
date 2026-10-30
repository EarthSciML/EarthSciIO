//! The `zarr` **writer** — a sharded Zarr **v3** output backend built on the
//! mainstream [`zarrs`] crate (the write mirror of [`ZarrReader`](super::ZarrReader)).
//!
//! It emits the same store shape the Julia reference writer does
//! (`julia/src/zarr_write.jl`, streaming-output-sinks RFC §16), so a store written
//! by any track is cross-readable by the others:
//!
//!   * `zarr_format: 3`, one `zarr.json` per group + per array.
//!   * The **`sharding_indexed`** codec: the array's `chunk_grid.chunk_shape` is the
//!     SHARD (outer, write) shape; the sharding codec's `chunk_shape` is the inner
//!     (read) chunk. Inner pipeline `[bytes(little), blosc(zstd+shuffle)]`; index
//!     pipeline `[bytes(little), crc32c]`, `index_location: "end"`.
//!   * `dimension_names` + a mirrored CF `_ARRAY_DIMENSIONS` attribute; per-array
//!     `attributes` carry the caller's CF variable attributes; static coordinate
//!     arrays are written with values + attrs.
//!   * A per-store JSON output manifest (`output_manifest.json`, schema
//!     `earthsciio/output-manifest/v1`) mirroring `EarthSciIO`'s Julia writer.
//!
//! [`write_zarr_v3`] / [`write_all_to_store`] commit a whole dataset in one
//! call; they are thin wrappers over the streaming handle
//! [`ZarrStreamWriter`](super::ZarrStreamWriter) (`create / write_record /
//! flush / close`, plus regions, a commit protocol and indexed records —
//! `spec/writer.md`). `zarrs` owns the codec pipeline (blosc encode, crc32c,
//! shard assembly), so the crate's own code stays `#![forbid(unsafe_code)]`.
//!
//! **Tolerance policy.** Cross-language output is NOT byte-identical (Blosc
//! container bytes are host/version dependent). Conformance is on **decoded
//! arrays** within tolerance plus metadata equality (RFC §16).
//!
//! Local output uses `zarrs`' pure filesystem store; an object-storage target
//! (`s3://` — including S3-compatible endpoints — `gs://`, `az://`, `http(s)://`,
//! `memory://`) is handled by the scheme-dispatched `object_store`-backed opener
//! (feature `object-store`).

//! **Target note.** This module builds on `wasm32-unknown-unknown` too — it is
//! the one half of the crate that does (see `crate`'s module docs), because the
//! browser tier writes its run output with this writer rather than a fourth
//! implementation in JavaScript. Only [`write_zarr_v3`] is native-only, since a
//! local directory is what it writes into; [`write_all_to_store`] is the
//! store-generic entry every target shares, and on wasm32 the store is
//! [`OpfsStore`](super::OpfsStore).

use std::collections::BTreeMap;
#[cfg(not(target_arch = "wasm32"))]
use std::path::Path;
use std::sync::Arc;

use serde_json::{json, Map, Value};
#[cfg(not(target_arch = "wasm32"))]
use zarrs::filesystem::FilesystemStore;

use crate::error::{Error, Result};

/// A pinned Blosc codec profile: `cname` / `clevel` / byte-`shuffle`
/// (RFC §16). Kept as constants so the Julia/Python/Rust ports agree on params.
#[derive(Debug, Clone, Copy)]
pub struct BloscProfile {
    /// Blosc compressor name (e.g. `"zstd"`).
    pub cname: &'static str,
    /// Blosc compression level.
    pub clevel: u8,
    /// Byte-shuffle on/off.
    pub shuffle: bool,
}

/// Diagnostic profile — Blosc **zstd** + byte-shuffle, moderate level (5).
pub const BLOSC_DIAGNOSTIC: BloscProfile = BloscProfile {
    cname: "zstd",
    clevel: 5,
    shuffle: true,
};
/// Checkpoint profile — **lossless** Blosc zstd (level 7) + byte-shuffle.
pub const BLOSC_CHECKPOINT: BloscProfile = BloscProfile {
    cname: "zstd",
    clevel: 7,
    shuffle: true,
};

/// A pinned **standard Zarr v3 `zstd`** codec profile: `level` / `checksum`
/// (RFC §16). Unlike [`BloscProfile`] this is the plain v3 `zstd` codec, NOT the
/// Blosc container, so a WebAssembly/browser Zarr reader can decode the store:
/// the pure-Rust `zstd` codec targets `wasm32-unknown-unknown` whereas
/// `blosc-src`'s C sources do not. Lossless; no standalone byte-shuffle filter.
#[derive(Debug, Clone, Copy)]
pub struct ZstdProfile {
    /// Zstd compression level (−131072..=22; higher = smaller/slower).
    pub level: i32,
    /// Whether to store a content checksum in each frame.
    pub checksum: bool,
}

/// WASM profile — plain v3 **zstd** (level 5), no Blosc, so a wasm/browser Zarr
/// reader can decode the store. See [`ZstdProfile`].
pub const ZSTD_WASM: ZstdProfile = ZstdProfile {
    level: 5,
    checksum: false,
};

/// The inner (per-chunk) compressor a profile selects: Blosc(zstd) for the
/// diagnostic/checkpoint profiles, or the plain v3 zstd codec for the wasm one.
#[derive(Debug, Clone, Copy)]
pub enum CodecProfile {
    /// Blosc container (diagnostic / checkpoint).
    Blosc(BloscProfile),
    /// Plain v3 `zstd` codec (wasm-loadable).
    Zstd(ZstdProfile),
}

pub(super) fn profile(name: &str) -> Result<CodecProfile> {
    match name {
        "diagnostic" => Ok(CodecProfile::Blosc(BLOSC_DIAGNOSTIC)),
        "checkpoint" => Ok(CodecProfile::Blosc(BLOSC_CHECKPOINT)),
        "wasm" => Ok(CodecProfile::Zstd(ZSTD_WASM)),
        other => Err(err(format!(
            "unknown codec profile '{other}' (expected 'diagnostic', 'checkpoint', or 'wasm')"
        ))),
    }
}

/// One static (time-independent) coordinate array: 1-D over its own dim `name`,
/// with `values` and CF `attrs` (e.g. `units`, `standard_name`). `float64`.
#[derive(Debug, Clone)]
pub struct WriteCoord {
    /// Coordinate (and dimension) name.
    pub name: String,
    /// The coordinate values (1-D).
    pub values: Vec<f64>,
    /// CF attributes written into the array node's `attributes`.
    pub attrs: Map<String, Value>,
}

/// One streaming output variable: its on-disk `dims` (file order), CF `attrs`,
/// and its `data` flattened **row-major (C order)** over the shape implied by
/// `dims`. `float64`.
#[derive(Debug, Clone)]
pub struct WriteVar {
    /// Variable name.
    pub name: String,
    /// Ordered dimension names (must all be schema dims; includes `time_dim`).
    pub dims: Vec<String>,
    /// CF attributes (e.g. `units`, `coordinates`).
    pub attrs: Map<String, Value>,
    /// Row-major (C-order) values over `dims`.
    pub data: Vec<f64>,
}

/// The output schema (a whole-dataset write). Mirrors the Julia `OutputSchema`.
#[derive(Debug, Clone)]
pub struct OutputSchema {
    /// ORDERED dim name => length (the `time_dim` entry's length is the number of
    /// records written).
    pub dims: Vec<(String, usize)>,
    /// The growable/record axis name.
    pub time_dim: String,
    /// dim name => INNER chunk length (every dim + coord name needs an entry).
    pub chunk_shape: BTreeMap<String, usize>,
    /// dim name => SHARD length (a multiple of the inner chunk length per dim).
    pub shard_shape: BTreeMap<String, usize>,
    /// ORDERED static coordinate arrays (written once, with values).
    pub coords: Vec<WriteCoord>,
    /// ORDERED streaming variables.
    pub vars: Vec<WriteVar>,
    /// Group-level attributes.
    pub group_attrs: Map<String, Value>,
    /// Codec profile name: `"diagnostic"` / `"checkpoint"` (Blosc-zstd inner
    /// codec) or `"wasm"` (plain v3 zstd inner codec, no Blosc — wasm-loadable).
    pub profile: String,
}

pub(super) fn err(detail: impl Into<String>) -> Error {
    Error::Format {
        format: "zarr".to_string(),
        detail: detail.into(),
    }
}

/// Write a complete sharded Zarr **v3** store at `base` (a local directory)
/// following [`OutputSchema`], plus the JSON output manifest. `zarrs` performs
/// all codec work (blosc, crc32c, shard packing).
///
/// # Errors
/// Returns [`Error::Format`] on schema inconsistency (e.g. a shard shape not a
/// multiple of the inner chunk), or [`Error::Io`] / a `zarrs` error on write.
#[cfg(not(target_arch = "wasm32"))]
pub fn write_zarr_v3(base: &Path, schema: &OutputSchema) -> Result<()> {
    let _ = profile(&schema.profile)?; // fail fast on a bad profile before any I/O
    std::fs::create_dir_all(base).map_err(|e| Error::io(Some(base.to_path_buf()), e))?;
    let store = Arc::new(
        FilesystemStore::new(base)
            .map_err(|e| err(format!("open filesystem store at {}: {e}", base.display())))?,
    );
    write_all_to_store(store, &base.to_string_lossy(), schema)
}

/// Write group + coordinate/variable arrays + output manifest into any `zarrs`
/// read/write store.
///
/// **This is the writer.** Every target-specific entry point is a thin wrapper
/// that chooses the store and then calls this: [`write_zarr_v3`] (a local
/// directory), `write_zarr_object_store` (`s3://`, `http(s)://`, `file://`, …,
/// feature `object-store`), and `write_zarr_opfs` (the browser's Origin Private
/// File System, feature `opfs`). That is what makes "one writer, several hosts"
/// true by construction rather than by inspection — a store written from a
/// browser tab and one written by a server differ only in where the bytes land.
///
/// It is itself a thin wrapper over the streaming handle
/// ([`ZarrStreamWriter`](super::ZarrStreamWriter)): `create(Single)`, one
/// `write_record` per time step, `close` — so the whole-dataset and the
/// streaming paths are one code path (`spec/writer.md` §9). A time step's `t`
/// is the time coordinate's value when `coords` carries one for `time_dim`,
/// else the record index. A time coordinate must therefore hold exactly
/// `dims[time_dim]` strictly increasing values.
///
/// `base_url` is recorded verbatim in the output manifest's `base_url` field.
///
/// # Errors
/// [`Error::Format`] on a schema inconsistency (a shard shape that is not a
/// multiple of the inner chunk, a variable whose data length disagrees with its
/// dims, a time coordinate of the wrong length or not strictly increasing) or a
/// `zarrs` encode/store failure.
pub fn write_all_to_store<S>(store: Arc<S>, base_url: &str, schema: &OutputSchema) -> Result<()>
where
    S: zarrs::storage::ReadableWritableStorageTraits + 'static,
{
    use super::zarr_stream::{
        Region, Role, StreamDType, StreamSchema, StreamVar, TimeMode, ZarrStreamWriter,
    };

    let _ = profile(&schema.profile)?;
    let dim_len: BTreeMap<&str, usize> =
        schema.dims.iter().map(|(k, v)| (k.as_str(), *v)).collect();
    let n_rec = dim_len.get(schema.time_dim.as_str()).copied().unwrap_or(0);

    // Validate each variable's data against its dims (the stream schema
    // carries no data, so this check stays here).
    let mut layouts = Vec::with_capacity(schema.vars.len());
    for v in &schema.vars {
        let ti = v
            .dims
            .iter()
            .position(|d| d == &schema.time_dim)
            .ok_or_else(|| {
                err(format!(
                    "streaming var '{}' must include the time dim '{}'",
                    v.name, schema.time_dim
                ))
            })?;
        let shape: Vec<usize> =
            v.dims
                .iter()
                .map(|d| {
                    dim_len.get(d.as_str()).copied().ok_or_else(|| {
                        err(format!("var '{}' dim '{d}' not in schema dims", v.name))
                    })
                })
                .collect::<Result<_>>()?;
        let expect: usize = shape.iter().product();
        if v.data.len() != expect {
            return Err(err(format!(
                "var '{}' data length {} != product(shape {:?}) = {expect}",
                v.name,
                v.data.len(),
                shape
            )));
        }
        layouts.push((ti, shape));
    }

    let stream = StreamSchema {
        dims: schema.dims.clone(),
        time_dim: schema.time_dim.clone(),
        chunk_shape: schema.chunk_shape.clone(),
        shard_shape: schema.shard_shape.clone(),
        coords: schema.coords.clone(),
        vars: schema
            .vars
            .iter()
            .map(|v| StreamVar {
                name: v.name.clone(),
                dims: v.dims.clone(),
                dtype: StreamDType::F64,
                attrs: v.attrs.clone(),
            })
            .collect(),
        statics: Vec::new(),
        time_mode: TimeMode::Append,
        group_attrs: schema.group_attrs.clone(),
        profile: schema.profile.clone(),
    };
    let times: Option<&Vec<f64>> = schema
        .coords
        .iter()
        .find(|c| c.name == schema.time_dim)
        .map(|c| &c.values);
    // Each record's t comes from the time coordinate, and append-mode t must
    // be strictly increasing (`spec/writer.md` §6.1): check it whole, before
    // any I/O, rather than failing partway or mixing in record indices.
    if let Some(tv) = times {
        if tv.len() != n_rec {
            return Err(err(format!(
                "time coordinate '{}' has {} values but dims['{}'] = {n_rec}",
                schema.time_dim,
                tv.len(),
                schema.time_dim
            )));
        }
        if let Some(t) = tv.iter().find(|t| t.is_nan()) {
            return Err(err(format!(
                "time coordinate '{}' holds {t}",
                schema.time_dim
            )));
        }
        // NaN was rejected above, so `<=` is the exact negation of `>` here.
        if let Some(i) = (1..tv.len()).find(|&i| tv[i] <= tv[i - 1]) {
            return Err(err(format!(
                "time coordinate '{}' must be strictly increasing: value {i} = {} is not after value {} = {}",
                schema.time_dim,
                tv[i],
                i - 1,
                tv[i - 1]
            )));
        }
    }

    let mut w = ZarrStreamWriter::create(store, base_url, stream, Role::Single, Region::whole())?;
    for r in 0..n_rec {
        // Slice record r out of each row-major variable: the outer dims before
        // the time axis, a fixed r, and the contiguous inner block after it.
        let slabs: Vec<Vec<f64>> = schema
            .vars
            .iter()
            .zip(&layouts)
            .map(|(v, (ti, shape))| {
                let outer: usize = shape[..*ti].iter().product();
                let inner: usize = shape[ti + 1..].iter().product();
                let mut out = Vec::with_capacity(outer * inner);
                for o in 0..outer {
                    let start = (o * shape[*ti] + r) * inner;
                    out.extend_from_slice(&v.data[start..start + inner]);
                }
                out
            })
            .collect();
        let arrays: super::zarr_stream::RecordArrays = schema
            .vars
            .iter()
            .zip(&slabs)
            .map(|(v, s)| (v.name.clone(), s.as_slice().into()))
            .collect();
        #[allow(clippy::cast_precision_loss)]
        let t = times.map_or(r as f64, |tv| tv[r]);
        w.write_record(t, &arrays)?;
    }
    w.close(&[])?;
    Ok(())
}

/// The inner (per-chunk) compressor codec dict a profile selects: Blosc(zstd)
/// for the diagnostic/checkpoint profiles, or the plain v3 `zstd` codec for the
/// wasm profile (no Blosc — a wasm/browser reader can decode it).
fn inner_compressor(codec: &CodecProfile, typesize: usize) -> Value {
    match codec {
        CodecProfile::Blosc(b) => json!({
            "name": "blosc", "configuration": {
                "cname": b.cname,
                "clevel": b.clevel,
                "shuffle": if b.shuffle { "shuffle" } else { "noshuffle" },
                "typesize": typesize,
                "blocksize": 0,
            }
        }),
        CodecProfile::Zstd(z) => json!({
            "name": "zstd", "configuration": {
                "level": z.level,
                "checksum": z.checksum,
            }
        }),
    }
}

/// The `sharding_indexed` codec dict for inner chunk shape `inner` (RFC §16).
pub(super) fn sharding_codec(inner: &[usize], codec: &CodecProfile, typesize: usize) -> Value {
    json!({
        "name": "sharding_indexed",
        "configuration": {
            "chunk_shape": inner,
            "codecs": [
                {"name": "bytes", "configuration": {"endian": "little"}},
                inner_compressor(codec, typesize),
            ],
            "index_codecs": [
                {"name": "bytes", "configuration": {"endian": "little"}},
                {"name": "crc32c"},
            ],
            "index_location": "end",
        },
    })
}

/// Store a pretty-JSON document at store key `key` (e.g. `zarr.json`,
/// `output_manifest.json`).
pub(super) fn set_json<S>(store: &S, key: &str, value: &Value) -> Result<()>
where
    S: zarrs::storage::WritableStorageTraits,
{
    let bytes =
        serde_json::to_vec_pretty(value).map_err(|e| err(format!("serialize {key}: {e}")))?;
    let store_key = zarrs::storage::StoreKey::new(key)
        .map_err(|e| err(format!("invalid store key '{key}': {e}")))?;
    store
        .set(&store_key, zarrs::storage::Bytes::from(bytes))
        .map_err(|e| err(format!("write {key}: {e}")))
}
