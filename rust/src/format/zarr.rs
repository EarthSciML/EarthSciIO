//! The `zarr` reader — a **store-backed** chunked-array reader built on the
//! mainstream [`zarrs`] crate (Zarr v3 **and** v2 read).
//!
//! A Zarr store is not one blob: each array's metadata (`zarr.json` for v3,
//! `.zarray`/`.zattrs` for v2) and every chunk/shard is its **own object with its
//! own URL**, so "lazy partial read" is "fetch only the chunk objects the
//! selection intersects". This reader declares itself
//! [`store_backed`](super::Reader::store_backed): the [`crate::Provider`] hands it
//! `(cache, base_url, variables, select)` and each object it needs is fetched
//! through the content-addressed [`Cache`] via [`CacheStorage`] — reusing the
//! offline / HTTP / S3 transport, integrity, and locking path of every other blob.
//!
//! **Decode.** Chunk decode (blosc containers, byte-shuffle, the v3
//! `sharding_indexed` codec, crc32c, zstd, …) is `zarrs`' job — this replaces the
//! former hand-rolled blosc1 container decoder. `zarrs` reads the pinned ISRM
//! corpus store (Zarr **v2**, blosc-lz4) by converting the v2 compressor metadata
//! to a v3 codec chain on open. The crate's own code remains `#![forbid(unsafe_code)]`
//! (that governs this crate only; `zarrs`' codec deps use `unsafe` internally,
//! which is fine).
//!
//! **Laziness (Risk: over-fetch).** The orthogonal selection is resolved to a set
//! of intersecting **chunk ids** (the Cartesian product of the per-axis chunk-id
//! sets); only those chunks are retrieved, one `zarrs` subset read per chunk. A
//! non-selected chunk object is never fetched (the corpus laziness test poisons
//! the unselected chunks to prove it). `fill_value` is **not** mapped to NaN (0.0
//! is real ISRM data); `zarrs` fills only an **absent** chunk object's region.

use std::collections::{BTreeSet, HashMap};
use std::path::Path;
use std::sync::Arc;

use zarrs::array::Array;
use zarrs::plugin::{ExtensionName, ZarrVersion};

use super::zarr_store::CacheStorage;
use super::{ArrayData, AxisSelect, DType, NativeDataset, NativeField, Reader, Selection};
use crate::cache::Cache;
use crate::error::{Error, Result};

/// The store-backed `zarr` reader (Zarr v3 + v2 chunked arrays, via `zarrs`).
pub struct ZarrReader;

impl ZarrReader {
    /// Construct a `zarr` reader.
    pub fn new() -> Self {
        ZarrReader
    }
}

impl Default for ZarrReader {
    fn default() -> Self {
        Self::new()
    }
}

fn zarr_err(detail: impl Into<String>) -> Error {
    Error::Format {
        format: "zarr".to_string(),
        detail: detail.into(),
    }
}

impl Reader for ZarrReader {
    fn formats(&self) -> &'static [&'static str] {
        &["zarr"]
    }

    fn extensions(&self) -> &'static [&'static str] {
        &["zarr"]
    }

    fn read_native(
        &self,
        _blob: &Path,
        _vars: &[String],
        _sel: &Selection,
    ) -> Result<NativeDataset> {
        Err(zarr_err(
            "zarr is store-backed; the Provider calls read_store",
        ))
    }

    fn store_backed(&self) -> bool {
        true
    }

    fn supports_selection(&self) -> bool {
        true
    }

    /// The full (dims-order) shape of `var`, read from ONLY its metadata object
    /// (`zarr.json`/`.zarray`, never a chunk) by opening the `zarrs` array.
    fn array_shape(
        &self,
        cache: Arc<Cache>,
        base_url: &str,
        var: &str,
    ) -> Result<Option<Vec<usize>>> {
        let storage = Arc::new(CacheStorage::new(cache, base_url));
        let array = open_array(storage, var)?;
        Ok(Some(array.shape().iter().map(|&s| s as usize).collect()))
    }

    fn read_store(
        &self,
        cache: Arc<Cache>,
        base_url: &str,
        variables: &[String],
        select: &Selection,
    ) -> Result<NativeDataset> {
        if variables.is_empty() {
            return Err(zarr_err(
                "the zarr reader requires an explicit list of variables (arrays); \
                 the store cannot be enumerated without a consolidated metadata index",
            ));
        }
        let storage = Arc::new(CacheStorage::new(cache, base_url));
        let axes: Option<&[AxisSelect]> = match select {
            Selection::Orthogonal(a) => Some(a.as_slice()),
            _ => None,
        };
        read_arrays(storage, variables, axes)
    }
}

/// Decode `variables` from an already-constructed `zarrs` storage (any backend:
/// the content-addressed [`CacheStorage`] or, under the `object-store` feature, an
/// object-store adapter). `axes` is the per-axis orthogonal selection applied to
/// arrays whose rank matches; `None` (or a rank mismatch) reads the whole array.
/// Only the chunk objects the selection intersects are retrieved (lazy).
pub(crate) fn read_arrays<S>(
    storage: Arc<S>,
    variables: &[String],
    axes: Option<&[AxisSelect]>,
) -> Result<NativeDataset>
where
    S: zarrs::storage::ReadableStorageTraits + 'static,
{
    let mut out_vars = HashMap::new();
    for array_name in variables {
        let array = open_array(storage.clone(), array_name)?;
            let shape: Vec<usize> = array.shape().iter().map(|&s| s as usize).collect();
            let ndim = shape.len();

            // Regular chunk grid: the chunk shape is uniform, so read it off chunk 0.
            let zeros = vec![0u64; ndim];
            let chunk_shape: Vec<usize> = array
                .chunk_shape_usize(&zeros)
                .map_err(|e| zarr_err(format!("chunk shape of '{array_name}': {e}")))?;

            let dims = dim_names(&array, ndim);
            let (dtype, is_f32) = float_dtype(&array, array_name)?;

            // Resolve per-axis global index lists (ndim-match on the selection).
            let sel_idx: Vec<Vec<usize>> = match axes {
                Some(a) if a.len() == ndim => {
                    let mut v = Vec::with_capacity(ndim);
                    for d in 0..ndim {
                        v.push(a[d].resolve(shape[d])?);
                    }
                    v
                }
                _ => (0..ndim).map(|d| (0..shape[d]).collect()).collect(),
            };

            // Fetch + decode ONLY the chunk objects the selection intersects.
            let mut chunks: HashMap<Vec<usize>, ChunkBuf> = HashMap::new();
            for cid in needed_chunks(&sel_idx, &chunk_shape) {
                let cid_u64: Vec<u64> = cid.iter().map(|&c| c as u64).collect();
                let subset = array
                    .chunk_subset(&cid_u64)
                    .map_err(|e| zarr_err(format!("chunk subset {cid:?} of '{array_name}': {e}")))?;
                let cstart: Vec<usize> = subset.start().iter().map(|&s| s as usize).collect();
                let cshape: Vec<usize> = subset.shape().iter().map(|&s| s as usize).collect();
                let elems = if is_f32 {
                    array
                        .retrieve_array_subset::<Vec<f32>>(&subset)
                        .map_err(|e| zarr_err(format!("decode chunk {cid:?} of '{array_name}': {e}")))?
                        .into_iter()
                        .map(|x| x as f64)
                        .collect::<Vec<f64>>()
                } else {
                    array
                        .retrieve_array_subset::<Vec<f64>>(&subset)
                        .map_err(|e| zarr_err(format!("decode chunk {cid:?} of '{array_name}': {e}")))?
                };
                chunks.insert(cid, ChunkBuf { cstart, cshape, elems });
            }

            let data = assemble(&sel_idx, &chunk_shape, &chunks);
            let out_shape: Vec<usize> = sel_idx.iter().map(Vec::len).collect();
            out_vars.insert(
                array_name.clone(),
                NativeField {
                    dtype,
                    dims,
                    shape: out_shape,
                    data: ArrayData::F64(data),
                    fill_value: None,
                },
            );
    }
    Ok(NativeDataset {
        variables: out_vars,
        coords: HashMap::new(),
    })
}

/// One retrieved chunk: its global start, its (boundary-clipped) shape, and its
/// C-order `f64` elements.
struct ChunkBuf {
    cstart: Vec<usize>,
    cshape: Vec<usize>,
    elems: Vec<f64>,
}

/// Open the `zarrs` array `name` under the store root. The array path is
/// `/<name>` (group-root-relative); this fetches only the metadata object.
fn open_array<S>(storage: Arc<S>, name: &str) -> Result<Array<S>>
where
    S: zarrs::storage::ReadableStorageTraits + 'static,
{
    let path = format!("/{}", name.trim_start_matches('/'));
    Array::open(storage, &path).map_err(|e| zarr_err(format!("open zarr array '{name}': {e}")))
}

/// Dimension names, preferring v3 `dimension_names`, then the v2/CF
/// `_ARRAY_DIMENSIONS` attribute, then synthesized `dim_0…`.
fn dim_names<S>(array: &Array<S>, ndim: usize) -> Vec<String> {
    if let Some(dn) = array.dimension_names() {
        let names: Vec<String> = dn.iter().filter_map(|d| d.clone()).collect();
        if names.len() == ndim {
            return names;
        }
    }
    if let Some(arr) = array
        .attributes()
        .get("_ARRAY_DIMENSIONS")
        .and_then(|v| v.as_array())
    {
        let names: Vec<String> = arr
            .iter()
            .filter_map(|x| x.as_str().map(String::from))
            .collect();
        if names.len() == ndim {
            return names;
        }
    }
    (0..ndim).map(|i| format!("dim_{i}")).collect()
}

/// The logical [`DType`] plus whether the on-disk element is `float32` (so the
/// caller widens to `f64`). Only float dtypes are supported (the pinned ISRM
/// store + fixture are `<f4`/`<f8`); integer dtypes error clearly, matching the
/// former reader.
fn float_dtype<S>(array: &Array<S>, name: &str) -> Result<(DType, bool)> {
    let dt = array.data_type();
    let n = dt.name(ZarrVersion::V3).map(|c| c.to_string()).unwrap_or_default();
    match n.as_str() {
        "float64" => Ok((DType::Float64, false)),
        "float32" => Ok((DType::Float64, true)),
        other => Err(zarr_err(format!(
            "the Rust zarr reader currently supports float dtypes (float32/float64); \
             array '{name}' has data type '{other}'"
        ))),
    }
}

// --- chunk math ------------------------------------------------------------- //

/// The SET of chunk-id tuples the orthogonal selection intersects (Cartesian
/// product of the per-dim chunk-id sets) — the crux of laziness.
fn needed_chunks(sel_idx: &[Vec<usize>], chunks: &[usize]) -> Vec<Vec<usize>> {
    let per_dim: Vec<Vec<usize>> = sel_idx
        .iter()
        .zip(chunks)
        .map(|(idxs, &cl)| {
            let set: BTreeSet<usize> = idxs.iter().map(|g| g / cl).collect();
            set.into_iter().collect()
        })
        .collect();
    let mut out: Vec<Vec<usize>> = vec![Vec::new()];
    for dim in &per_dim {
        let mut next = Vec::with_capacity(out.len() * dim.len());
        for prefix in &out {
            for &c in dim {
                let mut p = prefix.clone();
                p.push(c);
                next.push(p);
            }
        }
        out = next;
    }
    out
}

/// Scatter the retrieved chunk buffers into the C-order output over the selection
/// shape. Every selected position lies in some retrieved chunk; a position with
/// no covering chunk stays at 0.0 (never reached for a valid selection).
fn assemble(
    sel_idx: &[Vec<usize>],
    chunk_shape: &[usize],
    chunks: &HashMap<Vec<usize>, ChunkBuf>,
) -> Vec<f64> {
    let ndim = sel_idx.len();
    let sel_shape: Vec<usize> = sel_idx.iter().map(Vec::len).collect();
    let total: usize = sel_shape.iter().product();
    let mut data = vec![0.0f64; total];
    if total == 0 {
        return data;
    }
    for lin in 0..total {
        // C-order multi-index of this output position.
        let mut rem = lin;
        let mut midx = vec![0usize; ndim];
        for d in (0..ndim).rev() {
            midx[d] = rem % sel_shape[d];
            rem /= sel_shape[d];
        }
        let global: Vec<usize> = (0..ndim).map(|d| sel_idx[d][midx[d]]).collect();
        let cid: Vec<usize> = (0..ndim).map(|d| global[d] / chunk_shape[d]).collect();
        if let Some(buf) = chunks.get(&cid) {
            // C-order offset of the element within its (clipped) chunk subset.
            let mut off = 0usize;
            for d in 0..ndim {
                let w = global[d] - buf.cstart[d];
                off = off * buf.cshape[d] + w;
            }
            data[lin] = buf.elems[off];
        }
    }
    data
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn needed_chunks_orthogonal_dedup_and_skip() {
        // dim1 chunk_len 100: [0,250,260] -> chunks {0,2}; chunk 1 skipped.
        let got = needed_chunks(&[vec![1], vec![0, 250, 260], vec![0]], &[1, 100, 1]);
        assert_eq!(got, vec![vec![1, 0, 0], vec![1, 2, 0]]);
    }

    #[test]
    fn needed_chunks_never_scans_whole_array() {
        let got = needed_chunks(&[vec![0], vec![50, 12345, 52000], vec![0]], &[1, 100, 52411]);
        let dim1: BTreeSet<usize> = got.iter().map(|c| c[1]).collect();
        assert_eq!(dim1, BTreeSet::from([0, 123, 520]));
        assert_eq!(got.len(), 3); // never 525
    }

    #[test]
    fn axis_select_resolves() {
        assert_eq!(AxisSelect::All.resolve(4).unwrap(), vec![0, 1, 2, 3]);
        assert_eq!(
            AxisSelect::Indices(vec![3, 0, 1]).resolve(4).unwrap(),
            vec![3, 0, 1]
        );
        assert_eq!(
            AxisSelect::Range { start: 1, stop: 8, step: 2 }.resolve(10).unwrap(),
            vec![1, 3, 5, 7]
        );
        assert!(AxisSelect::Indices(vec![9]).resolve(4).is_err());
    }

    #[test]
    fn assemble_places_selected_indices() {
        // 1-D array, chunk_shape 2, one chunk (id 0) holding [10,20], select [1,0].
        let mut chunks = HashMap::new();
        chunks.insert(
            vec![0usize],
            ChunkBuf { cstart: vec![0], cshape: vec![2], elems: vec![10.0, 20.0] },
        );
        let out = assemble(&[vec![1, 0]], &[2], &chunks);
        assert_eq!(out, vec![20.0, 10.0]);
    }
}
