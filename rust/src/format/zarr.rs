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
//!
//! **Peak memory (Risk: amplification).** The output buffer is allocated FIRST and
//! each chunk is scattered into it **as it is decoded**, so exactly ONE decoded
//! chunk is live at a time. The reader used to collect every decoded chunk into a
//! `HashMap` and scatter afterwards, which held `output + ALL decoded chunks`: one
//! read of the real ISRM SR array (416 chunks x ~21 MB decompressed) peaked at
//! ~8.7 GB to produce a 0.59 GB result (~15x) and OOM-killed a production run.
//! Widening `float32` on disk to the `f64` output DOUBLED each retained buffer on
//! top of that, so the conversion is now done per element at scatter time rather
//! than per chunk up front. The scatter order changes; the result does not — every
//! output cell belongs to exactly one chunk id, so it is still written exactly
//! once, from the same value (`f32 as f64` is exact).

use std::borrow::Cow;
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

/// How many chunk objects to have in flight at once per array read.
///
/// Chunk fetches are latency-bound object GETs (or cache-hit file reads), so
/// the sweet spot is well above the CPU count but small enough that the
/// transient decoded-chunk buffers (one per worker) stay negligible next to the
/// output. `EARTHSCIIO_FETCH_CONCURRENCY` overrides; `1` restores the serial
/// loop. wasm has no threads, so it is always serial there.
fn fetch_concurrency() -> usize {
    #[cfg(target_arch = "wasm32")]
    {
        1
    }
    #[cfg(not(target_arch = "wasm32"))]
    {
        std::env::var("EARTHSCIIO_FETCH_CONCURRENCY")
            .ok()
            .and_then(|v| v.parse::<usize>().ok())
            .map(|v| v.clamp(1, 64))
            .unwrap_or(16)
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

    /// True only when the crate is built with the `object-store` feature — that
    /// is what supplies a storage backend which is not the cache. Reported
    /// honestly rather than optimistically, so a `Direct` loader in a build
    /// without the feature fails at the Provider with a message naming the
    /// feature, instead of erroring deep inside a read.
    fn supports_direct_read(&self) -> bool {
        cfg!(feature = "object-store")
    }

    #[cfg(feature = "object-store")]
    fn read_store_direct(
        &self,
        base_url: &str,
        variables: &[String],
        select: &Selection,
        options: &[(String, String)],
    ) -> Result<NativeDataset> {
        if variables.is_empty() {
            return Err(zarr_err(
                "the zarr reader requires an explicit list of variables (arrays); \
                 the store cannot be enumerated without a consolidated metadata index",
            ));
        }
        super::zarr_object_store::read_zarr_object_store_with_options(
            base_url,
            variables,
            select,
            &super::zarr_object_store::read_store_options(base_url, options),
        )
    }

    #[cfg(feature = "object-store")]
    fn array_shape_direct(
        &self,
        base_url: &str,
        var: &str,
        options: &[(String, String)],
    ) -> Result<Option<Vec<usize>>> {
        super::zarr_object_store::array_shape_object_store(
            base_url,
            var,
            &super::zarr_object_store::read_store_options(base_url, options),
        )
        .map(Some)
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

            // Allocate the output FIRST; a position no retrieved chunk covers stays
            // 0.0 (never reached for a valid selection). `fill_value` is NOT mapped
            // to NaN — zarrs already materializes it for an absent chunk object.
            let out_shape: Vec<usize> = sel_idx.iter().map(Vec::len).collect();
            let total: usize = out_shape.iter().product();
            let mut data = vec![0.0f64; total];
            let out_stride = c_strides(&out_shape);
            // Which output cells each chunk coordinate owns, per dimension.
            // Built ONCE per array — the mapping is chunk-invariant (a 52k-entry
            // list per "All" axis in the ISRM reads) — and shared read-only by
            // the workers; only the boundary-clipped chunk stride is per-chunk.
            let owners = chunk_owners(&sel_idx, &chunk_shape, &out_stride);

            // Fetch + decode ONLY the chunk objects the selection intersects, and
            // scatter each one into `data` immediately so it can be dropped before
            // the next is fetched (peak = output + one decoded chunk PER WORKER).
            //
            // Chunk objects are independent URLs fetched over a high-latency
            // transport, so they are retrieved CONCURRENTLY: a fixed pool of
            // scoped workers drains the chunk-id list, each fetching + decoding
            // with no lock held and taking the `data` mutex only for the (cheap)
            // scatter. Every output cell belongs to exactly one chunk id, so the
            // writes are disjoint and the result is byte-identical to the serial
            // order. A measured ISRM prepare spent ~85% of its wall time waiting
            // on these GETs one at a time.
            let cids: Vec<Vec<usize>> = needed_chunks(&sel_idx, &chunk_shape);
            let fetch_one = |cid: &Vec<usize>, out: &std::sync::Mutex<&mut [f64]>| -> Result<()> {
                let cid_u64: Vec<u64> = cid.iter().map(|&c| c as u64).collect();
                let subset = array
                    .chunk_subset(&cid_u64)
                    .map_err(|e| zarr_err(format!("chunk subset {cid:?} of '{array_name}': {e}")))?;
                let cshape: Vec<usize> = subset.shape().iter().map(|&s| s as usize).collect();
                // The `f32` buffer is scattered AS f32 (widened per element, which is
                // exact) — never materialized as a full `f64` chunk.
                if is_f32 {
                    let elems = array
                        .retrieve_array_subset::<Vec<f32>>(&subset)
                        .map_err(|e| zarr_err(format!("decode chunk {cid:?} of '{array_name}': {e}")))?;
                    let mut g = out.lock().expect("zarr scatter mutex poisoned");
                    scatter_chunk(&mut g, &elems, cid, &cshape, &owners);
                } else {
                    let elems = array
                        .retrieve_array_subset::<Vec<f64>>(&subset)
                        .map_err(|e| zarr_err(format!("decode chunk {cid:?} of '{array_name}': {e}")))?;
                    let mut g = out.lock().expect("zarr scatter mutex poisoned");
                    scatter_chunk(&mut g, &elems, cid, &cshape, &owners);
                }
                // `elems` drops HERE, before this worker fetches its next chunk.
                Ok(())
            };
            let workers = fetch_concurrency().min(cids.len());
            let out = std::sync::Mutex::new(&mut data[..]);
            if workers <= 1 {
                for cid in &cids {
                    fetch_one(cid, &out)?;
                }
            } else {
                use std::sync::atomic::{AtomicUsize, Ordering};
                let next = AtomicUsize::new(0);
                let fail: std::sync::Mutex<Option<Error>> = std::sync::Mutex::new(None);
                std::thread::scope(|s| {
                    for _ in 0..workers {
                        s.spawn(|| loop {
                            let i = next.fetch_add(1, Ordering::Relaxed);
                            if i >= cids.len() {
                                break;
                            }
                            if let Err(e) = fetch_one(&cids[i], &out) {
                                *fail.lock().expect("zarr error mutex poisoned") = Some(e);
                                // Claim every remaining chunk so the pool drains
                                // promptly after the first failure.
                                next.store(cids.len(), Ordering::Relaxed);
                                break;
                            }
                        });
                    }
                });
                if let Some(e) = fail.into_inner().expect("zarr error mutex poisoned") {
                    return Err(e);
                }
            }
            drop(out);

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

/// Open the `zarrs` array `name` under the store root. The array path is
/// `/<name>` (group-root-relative); this fetches only the metadata object.
pub(crate) fn open_array<S>(storage: Arc<S>, name: &str) -> Result<Array<S>>
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

/// C-order (row-major) strides for `shape`.
fn c_strides(shape: &[usize]) -> Vec<usize> {
    let mut st = vec![1usize; shape.len()];
    for d in (0..shape.len().saturating_sub(1)).rev() {
        st[d] = st[d + 1] * shape[d + 1];
    }
    st
}

/// Per dimension, which output positions each chunk coordinate owns: chunk coord
/// `c` maps to one `(output-linear offset, within-chunk index)` pair per output
/// position whose global index falls in chunk `c`. The output stride is already
/// applied and the chunk start (`c * chunk_len` — the regular grid the rest of
/// the chunk math also assumes) already subtracted, so this whole structure is
/// chunk-invariant: built once per array read, shared read-only by the fetch
/// workers, with only the boundary-clip-dependent chunk stride left for
/// [`scatter_chunk`] to apply.
///
/// This is what makes the scatter incremental. Every output position along a
/// dimension lies in EXACTLY ONE chunk coordinate (`global / chunk_len` is a
/// function), so the per-chunk Cartesian products of these lists **partition** the
/// output: scattering chunk-by-chunk writes every cell exactly once, the same
/// value the collect-then-scatter pass wrote, only in a different order. A
/// repeated index in the selection (e.g. `Indices([1, 1])`) is fine — each output
/// position appears once, so both copies are written.
fn chunk_owners(
    sel_idx: &[Vec<usize>],
    chunk_shape: &[usize],
    out_stride: &[usize],
) -> Vec<HashMap<usize, Vec<(usize, usize)>>> {
    sel_idx
        .iter()
        .zip(chunk_shape.iter().zip(out_stride))
        .map(|(idxs, (&cl, &os))| {
            let mut owned: HashMap<usize, Vec<(usize, usize)>> = HashMap::new();
            for (pos, &g) in idxs.iter().enumerate() {
                owned.entry(g / cl).or_default().push((pos * os, g % cl));
            }
            owned
        })
        .collect()
}

/// Scatter ONE decoded chunk into the C-order output over the selection shape,
/// then let the caller drop it. `elems` is the chunk's C-order elements over the
/// (boundary-clipped) subset with shape `cshape`; `f32` elements are widened per
/// element here (`f32 -> f64` is exact) so a `float32` chunk is never doubled
/// into an `f64` buffer.
fn scatter_chunk<T: Copy + Into<f64>>(
    out: &mut [f64],
    elems: &[T],
    cid: &[usize],
    cshape: &[usize],
    owners: &[HashMap<usize, Vec<(usize, usize)>>],
) {
    let ndim = cid.len();
    let cstride = c_strides(cshape);
    // Per dimension, the (output-linear, within-chunk-linear) contribution of every
    // output position this chunk owns along that dimension. `owners` already
    // carries both halves; only the chunk stride is applied here, and a stride
    // of 1 — always the last dimension, the large axis in the profiled reads —
    // needs no per-chunk rebuild at all.
    let mut per_dim: Vec<Cow<[(usize, usize)]>> = Vec::with_capacity(ndim);
    for d in 0..ndim {
        // A chunk id produced by `needed_chunks` always owns something on every
        // dimension; bail out rather than panic if that ever stops holding.
        let Some(owned) = owners[d].get(&cid[d]) else {
            return;
        };
        per_dim.push(if cstride[d] == 1 {
            Cow::Borrowed(owned.as_slice())
        } else {
            Cow::Owned(
                owned
                    .iter()
                    .map(|&(lin, local)| (lin, local * cstride[d]))
                    .collect(),
            )
        });
    }
    // Odometer over the Cartesian product of the per-dimension owner lists: one
    // output cell per combination. (`ndim == 0` writes the single scalar cell.)
    let mut counter = vec![0usize; ndim];
    loop {
        let mut lin = 0usize;
        let mut off = 0usize;
        for (dim, &c) in per_dim.iter().zip(counter.iter()) {
            let (l, o) = dim[c];
            lin += l;
            off += o;
        }
        out[lin] = elems[off].into();
        let mut d = ndim;
        loop {
            if d == 0 {
                return;
            }
            d -= 1;
            counter[d] += 1;
            if counter[d] < per_dim[d].len() {
                break;
            }
            counter[d] = 0;
        }
    }
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
    fn scatter_places_selected_indices() {
        // 1-D array, chunk_shape 2, one chunk (id 0) holding [10,20], select [1,0].
        let sel = vec![vec![1usize, 0]];
        let owners = chunk_owners(&sel, &[2], &c_strides(&[2]));
        let mut out = vec![0.0f64; 2];
        scatter_chunk(&mut out, &[10.0f64, 20.0], &[0], &[2], &owners);
        assert_eq!(out, vec![20.0, 10.0]);
    }

    // --- bit-identity vs the PREVIOUS (collect-then-scatter) implementation ---- //
    //
    // The reader used to decode EVERY needed chunk into a `HashMap<Vec<usize>,
    // ChunkBuf>` and scatter afterwards. That pass is preserved verbatim below as a
    // test-only ORACLE so the incremental scatter can be proved to produce the
    // exact same `f64` bits, not merely "close" values.

    /// One retrieved chunk, as the previous implementation retained it: global
    /// start, (boundary-clipped) shape, and C-order `f64` elements.
    struct ChunkBuf {
        cstart: Vec<usize>,
        cshape: Vec<usize>,
        elems: Vec<f64>,
    }

    /// The PREVIOUS `assemble`: scatter the (already fully collected) chunk buffers
    /// into the C-order output over the selection shape.
    ///
    /// Kept VERBATIM (index loops and all) so it is recognisably the code that
    /// shipped — hence the lint waiver rather than a tidier rewrite.
    #[allow(clippy::needless_range_loop)]
    fn assemble_reference(
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
            let mut rem = lin;
            let mut midx = vec![0usize; ndim];
            for d in (0..ndim).rev() {
                midx[d] = rem % sel_shape[d];
                rem /= sel_shape[d];
            }
            let global: Vec<usize> = (0..ndim).map(|d| sel_idx[d][midx[d]]).collect();
            let cid: Vec<usize> = (0..ndim).map(|d| global[d] / chunk_shape[d]).collect();
            if let Some(buf) = chunks.get(&cid) {
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

    /// A synthetic `float32` array value at a global multi-index — deliberately not
    /// f32-exact in decimal, so an f32/f64 rounding difference would show up.
    fn synthetic(global: &[usize]) -> f32 {
        let mut acc = 0.1f32;
        for (d, &g) in global.iter().enumerate() {
            acc += (g as f32) * (d as f32 + 1.0) * 0.3;
        }
        acc
    }

    /// The `zarrs` chunk subset of chunk `cid`: start, boundary-clipped shape, and
    /// the chunk's C-order `f32` elements from `synthetic`.
    fn synth_chunk(
        cid: &[usize],
        shape: &[usize],
        chunk_shape: &[usize],
    ) -> (Vec<usize>, Vec<usize>, Vec<f32>) {
        let ndim = shape.len();
        let cstart: Vec<usize> = (0..ndim).map(|d| cid[d] * chunk_shape[d]).collect();
        let cshape: Vec<usize> =
            (0..ndim).map(|d| chunk_shape[d].min(shape[d] - cstart[d])).collect();
        let n: usize = cshape.iter().product();
        let strides = c_strides(&cshape);
        let mut elems = vec![0.0f32; n];
        for (lin, slot) in elems.iter_mut().enumerate() {
            let global: Vec<usize> =
                (0..ndim).map(|d| cstart[d] + (lin / strides[d]) % cshape[d]).collect();
            *slot = synthetic(&global);
        }
        (cstart, cshape, elems)
    }

    /// Run BOTH paths over a synthetic multi-chunk `float32` array and return
    /// `(scatter_as_you_go, collect_then_scatter, cells_written)`.
    fn both_paths(
        shape: &[usize],
        chunk_shape: &[usize],
        sel_idx: &[Vec<usize>],
    ) -> (Vec<f64>, Vec<f64>, usize) {
        let cids = needed_chunks(sel_idx, chunk_shape);
        let out_shape: Vec<usize> = sel_idx.iter().map(Vec::len).collect();
        let total: usize = out_shape.iter().product();
        let out_stride = c_strides(&out_shape);
        let owners = chunk_owners(sel_idx, chunk_shape, &out_stride);

        // NEW: allocate first, scatter each chunk as it is "decoded", drop it.
        let mut new_out = vec![0.0f64; total];
        let mut written = 0usize;
        for cid in &cids {
            let (_cstart, cshape, elems) = synth_chunk(cid, shape, chunk_shape);
            written += (0..shape.len())
                .map(|d| owners[d].get(&cid[d]).map_or(0, Vec::len))
                .product::<usize>();
            scatter_chunk(&mut new_out, &elems, cid, &cshape, &owners);
            // `elems` drops here — the whole point.
        }

        // OLD: collect EVERY decoded chunk (widened to f64 up front), then scatter.
        let mut chunks: HashMap<Vec<usize>, ChunkBuf> = HashMap::new();
        for cid in &cids {
            let (cstart, cshape, elems) = synth_chunk(cid, shape, chunk_shape);
            let elems: Vec<f64> = elems.into_iter().map(|x| x as f64).collect();
            chunks.insert(cid.clone(), ChunkBuf { cstart, cshape, elems });
        }
        let old_out = assemble_reference(sel_idx, chunk_shape, &chunks);
        (new_out, old_out, written)
    }

    /// Assert the two paths agree BIT for bit (not within a tolerance).
    fn assert_bit_identical(new_out: &[f64], old_out: &[f64], ctx: &str) {
        assert_eq!(new_out.len(), old_out.len(), "{ctx}: length");
        for (i, (n, o)) in new_out.iter().zip(old_out).enumerate() {
            assert_eq!(n.to_bits(), o.to_bits(), "{ctx}: element {i}: {n} vs {o}");
        }
    }

    /// One sweep case: `(array shape, chunk shape, per-dim selected indices)`.
    type ScatterCase = (Vec<usize>, Vec<usize>, Vec<Vec<usize>>);

    #[test]
    fn scatter_as_you_go_is_bit_identical_to_collect_then_scatter() {
        // A sweep of multi-chunk shapes: ragged boundary chunks, permuted and
        // repeated indices, strided ranges, whole-array reads, rank 1..4.
        let cases: Vec<ScatterCase> = vec![
            // 3-D, ragged on every axis, permuted + repeated + reversed selections.
            (
                vec![7, 5, 11],
                vec![3, 2, 4],
                vec![vec![6, 0, 3, 3], vec![4, 1], vec![10, 0, 5, 5, 9]],
            ),
            // Whole-array read (many chunks, every cell selected).
            (vec![7, 5, 11], vec![3, 2, 4], vec![(0..7).collect(), (0..5).collect(), (0..11).collect()]),
            // A single index on the leading axis, all of the others (the ISRM shape).
            (vec![4, 9, 6], vec![1, 4, 3], vec![vec![2], (0..9).collect(), (0..6).collect()]),
            // Strided range.
            (vec![10, 10], vec![4, 3], vec![(0..10).step_by(3).collect(), vec![9, 2, 2, 0]]),
            // Rank 1, chunk larger than the array (one clipped chunk).
            (vec![5], vec![8], vec![vec![4, 0, 2]]),
            // Rank 1, chunk length 1 (one chunk per element).
            (vec![6], vec![1], vec![vec![5, 5, 0, 3]]),
            // Rank 4.
            (
                vec![3, 4, 2, 5],
                vec![2, 3, 1, 2],
                vec![vec![2, 0], vec![3, 1, 0], vec![1, 0], vec![4, 4, 1]],
            ),
            // Empty selection on one axis (zero-size output).
            (vec![4, 4], vec![2, 2], vec![vec![1, 2], vec![]]),
        ];

        for (shape, chunk_shape, sel_idx) in cases {
            let ctx = format!("shape={shape:?} chunks={chunk_shape:?} sel={sel_idx:?}");
            let (new_out, old_out, written) = both_paths(&shape, &chunk_shape, &sel_idx);
            assert_bit_identical(&new_out, &old_out, &ctx);
            // Each output cell is written EXACTLY once: the per-chunk cell counts
            // sum to the output size (they partition it — no double write, no gap).
            let total: usize = sel_idx.iter().map(Vec::len).product();
            assert_eq!(written, total, "{ctx}: cells written");
        }
    }

    #[test]
    fn scatter_visits_only_the_selected_chunks() {
        // The LAZINESS contract at the chunk-math level: making the scatter
        // incremental must not enlarge the set of chunk objects that get fetched.
        // Selecting y=1 and y=4 of a 5-row array chunked by 2 needs chunks 0 and 2
        // ONLY — chunk 1 (rows 2,3) is never visited.
        let sel_idx = vec![vec![1usize], vec![1, 4], vec![0, 1, 2, 3]];
        let chunk_shape = vec![1usize, 2, 4];
        let cids = needed_chunks(&sel_idx, &chunk_shape);
        assert_eq!(cids, vec![vec![1, 0, 0], vec![1, 2, 0]]);

        // And every visited chunk is genuinely used: it owns at least one output
        // cell on every dimension (a chunk fetched for nothing would be over-fetch).
        let owners = chunk_owners(&sel_idx, &chunk_shape, &c_strides(&[1, 2, 4]));
        for cid in &cids {
            for (d, own) in owners.iter().enumerate() {
                assert!(
                    own.get(&cid[d]).is_some_and(|v| !v.is_empty()),
                    "chunk {cid:?} owns nothing on dim {d}"
                );
            }
        }
        // The union of the owner lists is exactly the output, with no overlap.
        let cells: usize = cids
            .iter()
            .map(|cid| (0..3).map(|d| owners[d][&cid[d]].len()).product::<usize>())
            .sum();
        assert_eq!(cells, 8); // 1 layer x 2 rows x 4 cols
    }

    #[test]
    fn scatter_handles_f64_and_f32_identically() {
        // `f32 -> f64` widening is exact, so scattering an f32 chunk must equal
        // scattering the same values pre-widened to f64 (the old per-chunk convert).
        let shape = vec![5usize, 5];
        let chunk_shape = vec![2usize, 3];
        let sel_idx = vec![vec![4usize, 1, 0], vec![0, 3, 4]];
        let out_shape: Vec<usize> = sel_idx.iter().map(Vec::len).collect();
        let out_stride = c_strides(&out_shape);
        let owners = chunk_owners(&sel_idx, &chunk_shape, &out_stride);
        let total: usize = out_shape.iter().product();

        let mut from_f32 = vec![0.0f64; total];
        let mut from_f64 = vec![0.0f64; total];
        for cid in needed_chunks(&sel_idx, &chunk_shape) {
            let (_cstart, cshape, elems) = synth_chunk(&cid, &shape, &chunk_shape);
            let widened: Vec<f64> = elems.iter().map(|&x| x as f64).collect();
            scatter_chunk(&mut from_f32, &elems, &cid, &cshape, &owners);
            scatter_chunk(&mut from_f64, &widened, &cid, &cshape, &owners);
        }
        assert_bit_identical(&from_f32, &from_f64, "f32 vs pre-widened f64");
    }

    #[test]
    fn scatter_preserves_nan_and_zero_bits_verbatim() {
        // Values are MOVED, never arithmetic — a NaN payload and a signed zero must
        // survive the scatter unchanged (the reader does not map fill_value to NaN).
        let sel_idx = vec![vec![3usize, 0, 1, 2]];
        let chunk_shape = vec![4usize];
        let owners = chunk_owners(&sel_idx, &chunk_shape, &c_strides(&[4]));
        let elems = vec![-0.0f64, f64::NAN, f64::INFINITY, 0.0];
        let mut out = vec![0.0f64; 4];
        scatter_chunk(&mut out, &elems, &[0], &[4], &owners);
        assert_eq!(out[0].to_bits(), 0.0f64.to_bits()); // global 3
        assert_eq!(out[1].to_bits(), (-0.0f64).to_bits()); // global 0 — sign kept
        assert!(out[2].is_nan()); // global 1
        assert_eq!(out[3], f64::INFINITY); // global 2
    }
}
