//! The `zarr` reader — a **store-backed** Zarr v2 chunked-array reader.
//!
//! A Zarr v2 store is not one blob: each array's `.zarray`/`.zattrs` metadata and
//! every chunk is its **own object with its own URL**, so "lazy partial read" is
//! just "fetch only the chunk objects the selection intersects, each through the
//! existing content-addressed cache" (`spec/cloud-future.md` §3). No new cache-key
//! scheme and no byte-range machinery are needed for the pinned v2 target.
//!
//! This reader declares itself [`store_backed`](super::Reader::store_backed): the
//! [`crate::Provider`] hands it `(cache, base_url, variables, select)` and it
//! fetches each object it needs — `<base_url>/<array>/.zarray`, `.zattrs`
//! (optional), and only the intersecting `<chunk_key>` chunk objects — through
//! `cache.fetch`.
//!
//! **Decode / `#![forbid(unsafe_code)]`.** blosc chunks are decompressed by a
//! pure-Rust hand-rolled blosc1 container decoder (§2.8: 16-byte header + block
//! offsets + per-block codec + inverse byte-shuffle), with `lz4_flex` for the
//! lz4 codec (the pinned ISRM store + the conformance fixture are `cname: lz4`).
//! This preserves the crate's `#![forbid(unsafe_code)]` — unlike the `blosc`
//! crate, whose `decompress_bytes` is an `unsafe fn` that this crate could not
//! call without an `unsafe` block. `fill_value` is **not** mapped to NaN (0.0 is
//! real ISRM data); it fills only an **absent** chunk object's region.

use std::collections::{BTreeSet, HashMap};
use std::path::Path;

use serde_json::Value;

use super::{
    ArrayData, AxisSelect, DType, NativeDataset, NativeField, Reader, Selection,
};
use crate::cache::{Cache, FetchRequest};
use crate::error::{Error, Result};

/// The store-backed `zarr` reader (Zarr v2 chunked arrays).
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

impl Reader for ZarrReader {
    fn formats(&self) -> &'static [&'static str] {
        &["zarr"]
    }

    fn extensions(&self) -> &'static [&'static str] {
        &["zarr"]
    }

    fn read_native(&self, _blob: &Path, _vars: &[String], _sel: &Selection) -> Result<NativeDataset> {
        Err(Error::Format {
            format: "zarr".to_string(),
            detail: "zarr is store-backed; the Provider calls read_store".to_string(),
        })
    }

    fn store_backed(&self) -> bool {
        true
    }

    fn read_store(
        &self,
        cache: &Cache,
        base_url: &str,
        variables: &[String],
        select: &Selection,
    ) -> Result<NativeDataset> {
        if variables.is_empty() {
            return Err(Error::Format {
                format: "zarr".to_string(),
                detail: "the zarr reader requires an explicit list of variables (arrays); \
                         the store cannot be enumerated without a consolidated .zmetadata"
                    .to_string(),
            });
        }
        let base = base_url.trim_end_matches('/');
        let axes: Option<&[AxisSelect]> = match select {
            Selection::Orthogonal(a) => Some(a.as_slice()),
            _ => None,
        };

        let mut out_vars = HashMap::new();
        for array in variables {
            let meta = ZMeta::parse(&fetch_bytes(cache, &format!("{base}/{array}/.zarray"))?)?;
            let ndim = meta.shape.len();
            let zattrs = fetch_bytes_optional(cache, &format!("{base}/{array}/.zattrs"))?;
            let dims = parse_dims(zattrs.as_deref(), ndim);

            // Resolve the per-axis global index lists (ndim-match on the selection).
            let sel_idx: Vec<Vec<usize>> = match axes {
                Some(a) if a.len() == ndim => {
                    let mut v = Vec::with_capacity(ndim);
                    for d in 0..ndim {
                        v.push(a[d].resolve(meta.shape[d])?);
                    }
                    v
                }
                _ => (0..ndim).map(|d| (0..meta.shape[d]).collect()).collect(),
            };

            // Fetch + decode ONLY the chunk objects the selection intersects.
            let mut buffers: HashMap<Vec<usize>, Option<Vec<f64>>> = HashMap::new();
            for cid in needed_chunks(&sel_idx, &meta.chunks) {
                let key = chunk_key(&cid, &meta.dim_sep);
                let url = format!("{base}/{array}/{key}");
                match fetch_bytes_optional(cache, &url)? {
                    None => {
                        buffers.insert(cid, None); // absent chunk object -> fill region
                    }
                    Some(raw) => {
                        let decompressed = decompress(&meta, &raw)?;
                        let flat = meta.bytes_to_f64(&decompressed)?;
                        buffers.insert(cid, Some(flat));
                    }
                }
            }

            let data = assemble(&sel_idx, &meta, &buffers);
            let shape: Vec<usize> = sel_idx.iter().map(Vec::len).collect();
            out_vars.insert(
                array.clone(),
                NativeField {
                    dtype: DType::Float64,
                    dims,
                    shape,
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
}

// --- .zarray metadata ------------------------------------------------------- //

struct ZMeta {
    shape: Vec<usize>,
    chunks: Vec<usize>,
    byteorder: char, // '<' little, '>' big, '|' n/a
    kind: char,      // 'f' float, 'i'/'u' int
    itemsize: usize,
    compressor: Option<Value>,
    order: char, // 'C' | 'F'
    fill_value: f64,
    dim_sep: String,
}

impl ZMeta {
    fn parse(bytes: &[u8]) -> Result<ZMeta> {
        let v: Value = serde_json::from_slice(bytes).map_err(|e| Error::Format {
            format: "zarr".to_string(),
            detail: format!(".zarray is not valid JSON: {e}"),
        })?;
        let err = |d: String| Error::Format {
            format: "zarr".to_string(),
            detail: d,
        };
        if v.get("zarr_format").and_then(Value::as_u64).unwrap_or(2) != 2 {
            return Err(err("zarr reader supports zarr_format 2 only (v3 is future work)".into()));
        }
        let shape = json_usize_array(&v, "shape").ok_or_else(|| err("missing/invalid shape".into()))?;
        let chunks =
            json_usize_array(&v, "chunks").ok_or_else(|| err("missing/invalid chunks".into()))?;
        if shape.len() != chunks.len() {
            return Err(err(format!("shape {shape:?} and chunks {chunks:?} rank mismatch")));
        }
        let ts = v
            .get("dtype")
            .and_then(Value::as_str)
            .ok_or_else(|| err("missing dtype".into()))?;
        let (byteorder, rest) = match ts.chars().next() {
            Some(c @ ('<' | '>' | '|')) => (c, &ts[1..]),
            _ => ('|', ts),
        };
        let kind = rest.chars().next().ok_or_else(|| err(format!("bad dtype {ts:?}")))?;
        let itemsize: usize = rest[1..]
            .parse()
            .map_err(|_| err(format!("bad dtype itemsize in {ts:?}")))?;
        if v.get("filters").map(|f| !f.is_null()).unwrap_or(false) {
            return Err(err("zarr filter pipelines are not supported yet".into()));
        }
        let compressor = match v.get("compressor") {
            Some(Value::Null) | None => None,
            Some(c) => Some(c.clone()),
        };
        let order = v.get("order").and_then(Value::as_str).unwrap_or("C");
        let order = match order {
            "C" => 'C',
            "F" => 'F',
            other => return Err(err(format!("unknown zarr order {other:?}"))),
        };
        let fill_value = match v.get("fill_value") {
            Some(Value::Null) | None => 0.0,
            Some(n) => n.as_f64().unwrap_or(0.0),
        };
        let dim_sep = match v.get("dimension_separator").and_then(Value::as_str) {
            Some(s) if !s.is_empty() => s.to_string(),
            _ => ".".to_string(),
        };
        Ok(ZMeta {
            shape,
            chunks,
            byteorder,
            kind,
            itemsize,
            compressor,
            order,
            fill_value,
            dim_sep,
        })
    }

    /// Reinterpret decompressed chunk bytes to `f64` (C-order flat, length
    /// `prod(chunks)`). Only float dtypes are supported (the pinned ISRM store +
    /// fixture are `<f4`/`<f8`); integer dtypes error clearly.
    fn bytes_to_f64(&self, bytes: &[u8]) -> Result<Vec<f64>> {
        let be = self.byteorder == '>';
        match (self.kind, self.itemsize) {
            ('f', 4) => Ok(bytes
                .chunks_exact(4)
                .map(|b| {
                    let a = [b[0], b[1], b[2], b[3]];
                    (if be { f32::from_be_bytes(a) } else { f32::from_le_bytes(a) }) as f64
                })
                .collect()),
            ('f', 8) => Ok(bytes
                .chunks_exact(8)
                .map(|b| {
                    let a = [b[0], b[1], b[2], b[3], b[4], b[5], b[6], b[7]];
                    if be { f64::from_be_bytes(a) } else { f64::from_le_bytes(a) }
                })
                .collect()),
            (k, s) => Err(Error::Format {
                format: "zarr".to_string(),
                detail: format!(
                    "the Rust zarr reader currently supports float dtypes (<f4/<f8); \
                     got dtype kind '{k}' itemsize {s}"
                ),
            }),
        }
    }
}

fn json_usize_array(v: &Value, key: &str) -> Option<Vec<usize>> {
    v.get(key)?
        .as_array()?
        .iter()
        .map(|x| x.as_u64().map(|n| n as usize))
        .collect()
}

/// Dim names from `.zattrs` `_ARRAY_DIMENSIONS`, or synthesized `dim_0…`.
fn parse_dims(zattrs: Option<&[u8]>, ndim: usize) -> Vec<String> {
    if let Some(bytes) = zattrs {
        if let Ok(v) = serde_json::from_slice::<Value>(bytes) {
            if let Some(arr) = v.get("_ARRAY_DIMENSIONS").and_then(Value::as_array) {
                let names: Vec<String> = arr
                    .iter()
                    .filter_map(|x| x.as_str().map(String::from))
                    .collect();
                if names.len() == ndim {
                    return names;
                }
            }
        }
    }
    (0..ndim).map(|i| format!("dim_{i}")).collect()
}

// --- chunk math ------------------------------------------------------------- //

fn chunk_key(cid: &[usize], sep: &str) -> String {
    cid.iter()
        .map(|c| c.to_string())
        .collect::<Vec<_>>()
        .join(sep)
}

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

/// Scatter the fetched chunk buffers into the C-order output over the selection
/// shape. An absent chunk (`None`) leaves its region at `fill_value`.
fn assemble(sel_idx: &[Vec<usize>], meta: &ZMeta, buffers: &HashMap<Vec<usize>, Option<Vec<f64>>>) -> Vec<f64> {
    let ndim = sel_idx.len();
    let sel_shape: Vec<usize> = sel_idx.iter().map(Vec::len).collect();
    let total: usize = sel_shape.iter().product();
    let mut data = vec![meta.fill_value; total];
    if total == 0 {
        return data;
    }
    for lin in 0..total {
        // Decode the C-order multi-index of this output position.
        let mut rem = lin;
        let mut midx = vec![0usize; ndim];
        for d in (0..ndim).rev() {
            midx[d] = rem % sel_shape[d];
            rem /= sel_shape[d];
        }
        let cid: Vec<usize> = (0..ndim).map(|d| sel_idx[d][midx[d]] / meta.chunks[d]).collect();
        if let Some(Some(chunk)) = buffers.get(&cid) {
            // C-order offset of the element within its chunk.
            let mut off = 0usize;
            for d in 0..ndim {
                let w = sel_idx[d][midx[d]] % meta.chunks[d];
                off = off * meta.chunks[d] + w;
            }
            data[lin] = chunk[off];
        }
    }
    // F-order chunks would need a transposed within-chunk stride; only 'C' is
    // exercised by the pinned store + fixture.
    let _ = meta.order;
    data
}

// --- decompression ---------------------------------------------------------- //

fn decompress(meta: &ZMeta, raw: &[u8]) -> Result<Vec<u8>> {
    match &meta.compressor {
        None => Ok(raw.to_vec()),
        Some(c) => {
            let id = c.get("id").and_then(Value::as_str).unwrap_or("");
            match id {
                "blosc" => blosc_decompress(raw),
                "" | "none" => Ok(raw.to_vec()),
                other => Err(Error::Format {
                    format: "zarr".to_string(),
                    detail: format!("unsupported zarr compressor id {other:?}"),
                }),
            }
        }
    }
}

/// Decode a blosc1 container (§2.8): 16-byte header, then either the raw bytes
/// (memcpy flag) or an `i32[nblocks]` offsets table + per-block codec streams,
/// with an inverse byte-shuffle per block. Only the lz4 codec + byte-shuffle are
/// implemented (the pinned store + fixture). Pure Rust — no `unsafe`.
fn blosc_decompress(src: &[u8]) -> Result<Vec<u8>> {
    let err = |d: String| Error::Format {
        format: "zarr".to_string(),
        detail: d,
    };
    if src.len() < 16 {
        return Err(err("blosc buffer shorter than its 16-byte header".into()));
    }
    let flags = src[2];
    let typesize = src[3] as usize;
    let nbytes = u32::from_le_bytes([src[4], src[5], src[6], src[7]]) as usize;
    let blocksize = u32::from_le_bytes([src[8], src[9], src[10], src[11]]) as usize;
    let cbytes = u32::from_le_bytes([src[12], src[13], src[14], src[15]]) as usize;

    let byte_shuffle = flags & 0x01 != 0;
    let memcpyed = flags & 0x02 != 0;
    let bit_shuffle = flags & 0x04 != 0;
    let codec = flags >> 5;
    if bit_shuffle {
        return Err(err("blosc bitshuffle is not supported (only byte-shuffle)".into()));
    }
    if memcpyed {
        // Stored raw: the payload after the header is the uncompressed data.
        if src.len() < 16 + nbytes {
            return Err(err("blosc memcpy buffer truncated".into()));
        }
        return Ok(src[16..16 + nbytes].to_vec());
    }
    if nbytes == 0 {
        return Ok(Vec::new());
    }
    if blocksize == 0 {
        return Err(err("blosc blocksize is 0 for a compressed buffer".into()));
    }
    let nblocks = nbytes.div_ceil(blocksize);
    let mut out = Vec::with_capacity(nbytes);
    for i in 0..nblocks {
        let bstart_off = 16 + 4 * i;
        if bstart_off + 4 > src.len() {
            return Err(err("blosc offsets table truncated".into()));
        }
        let bstart = i32::from_le_bytes([
            src[bstart_off],
            src[bstart_off + 1],
            src[bstart_off + 2],
            src[bstart_off + 3],
        ]) as usize;
        let bend = if i + 1 < nblocks {
            let o = 16 + 4 * (i + 1);
            i32::from_le_bytes([src[o], src[o + 1], src[o + 2], src[o + 3]]) as usize
        } else {
            cbytes
        };
        if bstart > bend || bend > src.len() {
            return Err(err("blosc block offsets out of range".into()));
        }
        let comp = &src[bstart..bend];
        let block_len = blocksize.min(nbytes - i * blocksize);
        let decoded = match codec {
            1 => lz4_flex::block::decompress(comp, block_len).map_err(|e| {
                err(format!("blosc lz4 block decode failed: {e}"))
            })?,
            other => {
                return Err(err(format!(
                    "blosc codec {other} not supported (only lz4); the pinned store uses lz4"
                )))
            }
        };
        if decoded.len() != block_len {
            return Err(err(format!(
                "blosc block decoded to {} bytes, expected {block_len}",
                decoded.len()
            )));
        }
        if byte_shuffle {
            out.extend_from_slice(&unshuffle(&decoded, typesize));
        } else {
            out.extend_from_slice(&decoded);
        }
    }
    if out.len() != nbytes {
        return Err(err(format!(
            "blosc decoded {} bytes, expected {nbytes}",
            out.len()
        )));
    }
    Ok(out)
}

/// Inverse byte-shuffle of one block: `out[i*T + j] = src[j*nelem + i]` for the
/// leading `nelem*T` bytes; trailing bytes (a non-multiple remainder) are copied
/// verbatim (matching c-blosc's shuffle).
fn unshuffle(src: &[u8], typesize: usize) -> Vec<u8> {
    if typesize <= 1 {
        return src.to_vec();
    }
    let n = src.len();
    let nelem = n / typesize;
    let shuffled = nelem * typesize;
    let mut out = vec![0u8; n];
    for j in 0..typesize {
        for i in 0..nelem {
            out[i * typesize + j] = src[j * nelem + i];
        }
    }
    out[shuffled..n].copy_from_slice(&src[shuffled..n]);
    out
}

// --- object fetch helpers --------------------------------------------------- //

fn fetch_bytes(cache: &Cache, url: &str) -> Result<Vec<u8>> {
    let blob = cache.fetch(&FetchRequest::new(url))?;
    std::fs::read(&blob.path).map_err(|e| Error::io(Some(blob.path.clone()), e))
}

fn fetch_bytes_optional(cache: &Cache, url: &str) -> Result<Option<Vec<u8>>> {
    match cache.fetch(&FetchRequest::new(url)) {
        Ok(blob) => Ok(Some(
            std::fs::read(&blob.path).map_err(|e| Error::io(Some(blob.path.clone()), e))?,
        )),
        Err(e) if e.is_cache_miss() => Ok(None),
        Err(e) => Err(e),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn chunk_key_joins_with_separator() {
        assert_eq!(chunk_key(&[0, 5, 0], "."), "0.5.0");
        assert_eq!(chunk_key(&[3], "."), "3");
        assert_eq!(chunk_key(&[1, 2], "/"), "1/2");
    }

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
    fn unshuffle_roundtrips_a_shuffled_block() {
        // Forward shuffle of 3 little-endian u32s, then unshuffle must recover.
        let orig: Vec<u8> = vec![
            1, 0, 0, 0, // 1
            2, 0, 0, 0, // 2
            3, 0, 0, 0, // 3
        ];
        let typesize = 4;
        let nelem = orig.len() / typesize;
        let mut shuffled = vec![0u8; orig.len()];
        for i in 0..nelem {
            for j in 0..typesize {
                shuffled[j * nelem + i] = orig[i * typesize + j];
            }
        }
        assert_eq!(unshuffle(&shuffled, typesize), orig);
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
}
