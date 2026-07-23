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
//! Unlike the Julia writer's incremental `write_record!` streaming loop, this Rust
//! writer commits a whole dataset in one [`write_zarr_v3`] call (the streaming
//! record/flush handle is future work). `zarrs` owns the codec pipeline (blosc
//! encode, crc32c, shard assembly), so the crate's own code stays
//! `#![forbid(unsafe_code)]`.
//!
//! **Tolerance policy.** Cross-language output is NOT byte-identical (Blosc
//! container bytes are host/version dependent). Conformance is on **decoded
//! arrays** within tolerance plus metadata equality (RFC §16).
//!
//! Local output uses `zarrs`' pure filesystem store; an S3 (`s3://`) target is
//! handled by the `object_store`-backed opener (feature `object-store`).

use std::collections::BTreeMap;
use std::path::Path;
use std::sync::Arc;

use serde_json::{json, Map, Value};
use zarrs::array::{Array, ArrayMetadata};
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

fn profile(name: &str) -> Result<BloscProfile> {
    match name {
        "diagnostic" => Ok(BLOSC_DIAGNOSTIC),
        "checkpoint" => Ok(BLOSC_CHECKPOINT),
        other => Err(err(format!(
            "unknown codec profile '{other}' (expected 'diagnostic' or 'checkpoint')"
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
    /// Codec profile name: `"diagnostic"` or `"checkpoint"`.
    pub profile: String,
}

fn err(detail: impl Into<String>) -> Error {
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
pub fn write_zarr_v3(base: &Path, schema: &OutputSchema) -> Result<()> {
    let codec = profile(&schema.profile)?;
    let dim_len: BTreeMap<&str, usize> =
        schema.dims.iter().map(|(k, v)| (k.as_str(), *v)).collect();

    // Validate the chunk/shard grid.
    for (d, _) in &schema.dims {
        let c = *schema
            .chunk_shape
            .get(d)
            .ok_or_else(|| err(format!("dim '{d}' missing from chunk_shape")))?;
        let s = *schema
            .shard_shape
            .get(d)
            .ok_or_else(|| err(format!("dim '{d}' missing from shard_shape")))?;
        if c == 0 || s == 0 || s % c != 0 {
            return Err(err(format!(
                "shard_shape[{d}]={s} must be a nonzero multiple of chunk_shape[{d}]={c}"
            )));
        }
    }

    std::fs::create_dir_all(base).map_err(|e| Error::io(Some(base.to_path_buf()), e))?;
    let store = Arc::new(
        FilesystemStore::new(base)
            .map_err(|e| err(format!("open filesystem store at {}: {e}", base.display())))?,
    );

    // Group metadata (zarr.json at the root).
    let group_meta = json!({
        "zarr_format": 3,
        "node_type": "group",
        "attributes": Value::Object(schema.group_attrs.clone()),
    });
    write_group_json(base, &group_meta)?;

    // Static coordinate arrays (values known now).
    for co in &schema.coords {
        let shape = vec![co.values.len()];
        write_array(
            store.clone(),
            &co.name,
            std::slice::from_ref(&co.name),
            &shape,
            schema,
            &codec,
            &co.attrs,
            &co.values,
        )?;
    }

    // Streaming variables.
    for v in &schema.vars {
        if !v.dims.iter().any(|d| d == &schema.time_dim) {
            return Err(err(format!(
                "streaming var '{}' must include the time dim '{}'",
                v.name, schema.time_dim
            )));
        }
        let shape: Vec<usize> = v
            .dims
            .iter()
            .map(|d| {
                dim_len
                    .get(d.as_str())
                    .copied()
                    .ok_or_else(|| err(format!("var '{}' dim '{d}' not in schema dims", v.name)))
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
        write_array(
            store.clone(),
            &v.name,
            &v.dims,
            &shape,
            schema,
            &codec,
            &v.attrs,
            &v.data,
        )?;
    }

    write_output_manifest(base, schema, &codec)?;
    Ok(())
}

/// The `sharding_indexed` codec dict for inner chunk shape `inner` (RFC §16).
fn sharding_codec(inner: &[usize], codec: &BloscProfile, typesize: usize) -> Value {
    json!({
        "name": "sharding_indexed",
        "configuration": {
            "chunk_shape": inner,
            "codecs": [
                {"name": "bytes", "configuration": {"endian": "little"}},
                {"name": "blosc", "configuration": {
                    "cname": codec.cname,
                    "clevel": codec.clevel,
                    "shuffle": if codec.shuffle { "shuffle" } else { "noshuffle" },
                    "typesize": typesize,
                    "blocksize": 0,
                }},
            ],
            "index_codecs": [
                {"name": "bytes", "configuration": {"endian": "little"}},
                {"name": "crc32c"},
            ],
            "index_location": "end",
        },
    })
}

/// Build the v3 array metadata dict (mirrors Julia `_array_meta_dict`).
fn array_meta(
    dims: &[String],
    shape: &[usize],
    schema: &OutputSchema,
    codec: &BloscProfile,
    attrs: &Map<String, Value>,
) -> Result<Value> {
    let inner: Vec<usize> = dims
        .iter()
        .map(|d| {
            schema
                .chunk_shape
                .get(d)
                .copied()
                .ok_or_else(|| err(format!("dim '{d}' missing from chunk_shape")))
        })
        .collect::<Result<_>>()?;
    let shard: Vec<usize> = dims
        .iter()
        .map(|d| {
            schema
                .shard_shape
                .get(d)
                .copied()
                .ok_or_else(|| err(format!("dim '{d}' missing from shard_shape")))
        })
        .collect::<Result<_>>()?;
    let mut a = attrs.clone();
    a.insert("_ARRAY_DIMENSIONS".to_string(), json!(dims));
    Ok(json!({
        "zarr_format": 3,
        "node_type": "array",
        "shape": shape,
        "data_type": "float64",
        "chunk_grid": {"name": "regular", "configuration": {"chunk_shape": shard}},
        "chunk_key_encoding": {"name": "default", "configuration": {"separator": "/"}},
        "fill_value": 0.0,
        "codecs": [sharding_codec(&inner, codec, std::mem::size_of::<f64>())],
        "attributes": Value::Object(a),
        "dimension_names": dims,
    }))
}

/// Create the array node (writing its `zarr.json`) and store its `data`.
#[allow(clippy::too_many_arguments)]
fn write_array(
    store: Arc<FilesystemStore>,
    name: &str,
    dims: &[String],
    shape: &[usize],
    schema: &OutputSchema,
    codec: &BloscProfile,
    attrs: &Map<String, Value>,
    data: &[f64],
) -> Result<()> {
    let meta_json = array_meta(dims, shape, schema, codec, attrs)?;
    let metadata: ArrayMetadata = serde_json::from_value(meta_json)
        .map_err(|e| err(format!("array '{name}' metadata is not valid v3: {e}")))?;
    let array = Array::new_with_metadata(store, &format!("/{name}"), metadata)
        .map_err(|e| err(format!("build zarr array '{name}': {e}")))?;
    array
        .store_metadata()
        .map_err(|e| err(format!("write metadata for '{name}': {e}")))?;
    // subset_all is the whole array; where the shard grid divides the shape evenly
    // this writes each shard exactly once (no read-modify-write).
    let subset = array.subset_all();
    array
        .store_array_subset(&subset, data)
        .map_err(|e| err(format!("write data for '{name}': {e}")))?;
    Ok(())
}

/// Write the group root `zarr.json` (pretty JSON, atomic via a sibling temp).
fn write_group_json(base: &Path, meta: &Value) -> Result<()> {
    let bytes = serde_json::to_vec_pretty(meta)
        .map_err(|e| err(format!("serialize group metadata: {e}")))?;
    let path = base.join("zarr.json");
    atomic_write(&path, &bytes)
}

/// Write the output manifest (`earthsciio/output-manifest/v1`) mirroring the
/// Julia `OutputManifest` fields.
fn write_output_manifest(base: &Path, schema: &OutputSchema, codec: &BloscProfile) -> Result<()> {
    let n_records = schema
        .dims
        .iter()
        .find(|(d, _)| d == &schema.time_dim)
        .map(|(_, l)| *l)
        .unwrap_or(0);
    let shard_time = schema
        .shard_shape
        .get(&schema.time_dim)
        .copied()
        .unwrap_or(n_records.max(1));

    // The time coordinate values (if a coord for the time dim was supplied) give
    // the per-shard t_start/t_end; otherwise fall back to record indices.
    let time_vals: Option<&Vec<f64>> = schema
        .coords
        .iter()
        .find(|c| c.name == schema.time_dim)
        .map(|c| &c.values);

    let mut time_shards = Vec::new();
    if shard_time > 0 && n_records > 0 {
        let mut index = 0usize;
        let mut start = 0usize;
        while start < n_records {
            let end = (start + shard_time).min(n_records);
            let n = end - start;
            let t_start = time_vals.and_then(|v| v.get(start).copied()).unwrap_or(start as f64);
            let t_end = time_vals
                .and_then(|v| v.get(end - 1).copied())
                .unwrap_or((end - 1) as f64);
            time_shards.push(json!({
                "index": index, "t_start": t_start, "t_end": t_end, "n": n,
            }));
            index += 1;
            start = end;
        }
    }
    let last_t: Value = if n_records == 0 {
        Value::Null
    } else {
        time_vals
            .and_then(|v| v.last().copied())
            .map(|t| json!(t))
            .unwrap_or_else(|| json!((n_records - 1) as f64))
    };

    let vars: Vec<Value> = schema
        .vars
        .iter()
        .map(|v| json!({"name": v.name, "dims": v.dims, "dtype": "float64"}))
        .collect();
    let dims: Vec<Value> = schema
        .dims
        .iter()
        .map(|(k, v)| json!({"name": k, "length": v}))
        .collect();

    let manifest = json!({
        "schema": "earthsciio/output-manifest/v1",
        "base_url": base.to_string_lossy(),
        "format": "zarr",
        "zarr_format": 3,
        "profile": schema.profile,
        "codec": {
            "id": "blosc",
            "cname": codec.cname,
            "clevel": codec.clevel,
            "shuffle": if codec.shuffle { "shuffle" } else { "noshuffle" },
        },
        "time_dim": schema.time_dim,
        "dims": dims,
        "vars": vars,
        "chunk_shape": schema.chunk_shape,
        "shard_shape": schema.shard_shape,
        "time_shards": time_shards,
        "last_t": last_t,
        "n_records": n_records,
        "created_at": crate::clock::now_rfc3339(),
    });
    let bytes = serde_json::to_vec_pretty(&manifest)
        .map_err(|e| err(format!("serialize output manifest: {e}")))?;
    atomic_write(&base.join("output_manifest.json"), &bytes)
}

/// Write `bytes` to `path` atomically via a sibling temp + rename.
fn atomic_write(path: &Path, bytes: &[u8]) -> Result<()> {
    let dir = path.parent().unwrap_or_else(|| Path::new("."));
    let mut tmp = tempfile::Builder::new()
        .prefix(".zarr-")
        .suffix(".part")
        .tempfile_in(dir)
        .map_err(|e| Error::io(Some(dir.to_path_buf()), e))?;
    use std::io::Write as _;
    tmp.write_all(bytes)
        .map_err(|e| Error::io(Some(tmp.path().to_path_buf()), e))?;
    tmp.flush()
        .map_err(|e| Error::io(Some(tmp.path().to_path_buf()), e))?;
    tmp.persist(path)
        .map_err(|e| Error::io(Some(path.to_path_buf()), e.error))?;
    Ok(())
}
