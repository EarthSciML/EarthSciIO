//! The streaming Zarr v3 **writer handle** — `create / write_record / flush /
//! close` — with region writes, a commit protocol, indexed records, statics,
//! blobs, typed arrays, read-back and resume (`spec/writer.md`, RFC U12).
//!
//! It emits the same sharded store as the Julia and Python writers (and as
//! [`write_all_to_store`](super::write_all_to_store), which is now a thin
//! wrapper over this handle):
//!
//!   * **Regions.** A handle is bound at open to a shard-aligned, 0-based,
//!     half-open block of the non-time dims ([`Region`]). Every shard object of
//!     a partitioned array therefore has exactly one writer: no read-modify-write
//!     and no locks, so it is correct on an object store too.
//!   * **Roles.** [`Role::Single`] writes and commits everything. With several
//!     writers, one [`Role::Owner`] writes every shared object (`zarr.json`,
//!     coordinates, replicated variables, the manifest) and the
//!     [`Role::Contributor`]s write only their region. Writers `flush` into a
//!     [`CommitToken`]; the caller (MPI, threads, a test) delivers the tokens to
//!     the owner, whose [`commit`](ZarrStreamWriter::commit) checks that they
//!     tile the extent and then writes shapes and the manifest, LAST. The
//!     manifest write is the commit point. This module knows nothing of MPI.
//!   * **Time modes.** [`TimeMode::Append`] (the record index is the running
//!     count) and [`TimeMode::Indexed`] (a fixed time axis; records written by
//!     index in any order, and rewritten after an owner `invalidate`).
//!
//! The writer encodes whole shards from its own buffers with `zarrs`'
//! `Array::store_chunk` and never writes a `zarr.json` it does not own: a
//! contributor encodes against an in-memory array whose shape covers the shard
//! it writes, which is never stored.
//!
//! Single-writer, append-only stores without statics keep writing
//! `earthsciio/output-manifest/v1`, unchanged; everything else writes `v2`.

use std::collections::{BTreeMap, BTreeSet};
use std::ops::Range;
use std::sync::Arc;

use serde::{Deserialize, Serialize};
use serde_json::{json, Map, Value};
use sha2::{Digest, Sha256};
use zarrs::array::{Array, ArrayMetadata, ArraySubset};
use zarrs::storage::{Bytes, ReadableStorageTraits, ReadableWritableStorageTraits, StoreKey};

use super::zarr_write::{err, profile, set_json, sharding_codec, CodecProfile, WriteCoord};
use crate::error::{Error, Result};

/// The v1 output-manifest label (single-writer append stores).
pub const OUTPUT_MANIFEST_V1: &str = "earthsciio/output-manifest/v1";
/// The v2 output-manifest label (regions, indexed records, statics, blobs).
pub const OUTPUT_MANIFEST_V2: &str = "earthsciio/output-manifest/v2";
/// The commit-token wire label.
pub const COMMIT_TOKEN_SCHEMA: &str = "earthsciio/commit-token/v1";
const MANIFEST_KEY: &str = "output_manifest.json";

// --- element types -----------------------------------------------------------

/// The element type of a streaming variable or static. (Named apart from the
/// reader's [`DType`](crate::DType), which describes decoded reads.)
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StreamDType {
    /// `float64`
    F64,
    /// `float32`
    F32,
    /// `int32`
    I32,
    /// `int64`
    I64,
    /// `uint8`
    U8,
}

impl StreamDType {
    /// The Zarr v3 `data_type` name.
    pub fn name(self) -> &'static str {
        match self {
            Self::F64 => "float64",
            Self::F32 => "float32",
            Self::I32 => "int32",
            Self::I64 => "int64",
            Self::U8 => "uint8",
        }
    }

    /// Parse a Zarr v3 `data_type` name.
    ///
    /// # Errors
    /// [`Error::Format`] for any other type.
    pub fn parse(s: &str) -> Result<Self> {
        Ok(match s {
            "float64" => Self::F64,
            "float32" => Self::F32,
            "int32" => Self::I32,
            "int64" => Self::I64,
            "uint8" => Self::U8,
            other => return Err(err(format!("unsupported stream dtype '{other}'"))),
        })
    }

    /// Bytes per element.
    pub fn size(self) -> usize {
        match self {
            Self::F64 | Self::I64 => 8,
            Self::F32 | Self::I32 => 4,
            Self::U8 => 1,
        }
    }

    fn is_float(self) -> bool {
        matches!(self, Self::F64 | Self::F32)
    }
}

/// A borrowed, row-major (C-order) typed array.
#[derive(Debug, Clone, Copy)]
pub enum TypedArray<'a> {
    /// `float64` values.
    F64(&'a [f64]),
    /// `float32` values.
    F32(&'a [f32]),
    /// `int32` values.
    I32(&'a [i32]),
    /// `int64` values.
    I64(&'a [i64]),
    /// `uint8` values.
    U8(&'a [u8]),
}

/// An owned, row-major (C-order) typed array (what the read-back API returns).
#[derive(Debug, Clone, PartialEq)]
pub enum TypedVec {
    /// `float64` values.
    F64(Vec<f64>),
    /// `float32` values.
    F32(Vec<f32>),
    /// `int32` values.
    I32(Vec<i32>),
    /// `int64` values.
    I64(Vec<i64>),
    /// `uint8` values.
    U8(Vec<u8>),
}

macro_rules! typed_from {
    ($t:ty, $v:ident) => {
        impl<'a> From<&'a [$t]> for TypedArray<'a> {
            fn from(s: &'a [$t]) -> Self {
                TypedArray::$v(s)
            }
        }
        impl<'a> From<&'a Vec<$t>> for TypedArray<'a> {
            fn from(s: &'a Vec<$t>) -> Self {
                TypedArray::$v(s.as_slice())
            }
        }
        impl From<Vec<$t>> for TypedVec {
            fn from(v: Vec<$t>) -> Self {
                TypedVec::$v(v)
            }
        }
    };
}
typed_from!(f64, F64);
typed_from!(f32, F32);
typed_from!(i32, I32);
typed_from!(i64, I64);
typed_from!(u8, U8);

fn ne_bytes<T: Copy, const N: usize>(s: &[T], f: impl Fn(T) -> [u8; N]) -> Vec<u8> {
    let mut out = Vec::with_capacity(s.len() * N);
    for &x in s {
        out.extend_from_slice(&f(x));
    }
    out
}

fn from_ne<T, const N: usize>(b: &[u8], f: impl Fn([u8; N]) -> T) -> Vec<T> {
    b.chunks_exact(N)
        .map(|c| f(c.try_into().expect("chunks_exact yields N bytes")))
        .collect()
}

impl TypedArray<'_> {
    /// The element type.
    pub fn dtype(&self) -> StreamDType {
        match self {
            Self::F64(_) => StreamDType::F64,
            Self::F32(_) => StreamDType::F32,
            Self::I32(_) => StreamDType::I32,
            Self::I64(_) => StreamDType::I64,
            Self::U8(_) => StreamDType::U8,
        }
    }

    /// Number of elements.
    pub fn len(&self) -> usize {
        match self {
            Self::F64(s) => s.len(),
            Self::F32(s) => s.len(),
            Self::I32(s) => s.len(),
            Self::I64(s) => s.len(),
            Self::U8(s) => s.len(),
        }
    }

    /// True when there are no elements.
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    fn to_ne_bytes(self) -> Vec<u8> {
        match self {
            Self::F64(s) => ne_bytes(s, f64::to_ne_bytes),
            Self::F32(s) => ne_bytes(s, f32::to_ne_bytes),
            Self::I32(s) => ne_bytes(s, i32::to_ne_bytes),
            Self::I64(s) => ne_bytes(s, i64::to_ne_bytes),
            Self::U8(s) => s.to_vec(),
        }
    }
}

impl TypedVec {
    /// The element type.
    pub fn dtype(&self) -> StreamDType {
        self.as_array().dtype()
    }

    /// Number of elements.
    pub fn len(&self) -> usize {
        self.as_array().len()
    }

    /// True when there are no elements.
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// Borrow as a [`TypedArray`].
    pub fn as_array(&self) -> TypedArray<'_> {
        match self {
            Self::F64(v) => TypedArray::F64(v),
            Self::F32(v) => TypedArray::F32(v),
            Self::I32(v) => TypedArray::I32(v),
            Self::I64(v) => TypedArray::I64(v),
            Self::U8(v) => TypedArray::U8(v),
        }
    }

    /// Every element widened to `f64` (exact for every type but large `int64`).
    pub fn to_f64(&self) -> Vec<f64> {
        match self {
            Self::F64(v) => v.clone(),
            Self::F32(v) => v.iter().map(|&x| f64::from(x)).collect(),
            Self::I32(v) => v.iter().map(|&x| f64::from(x)).collect(),
            #[allow(clippy::cast_precision_loss)]
            Self::I64(v) => v.iter().map(|&x| x as f64).collect(),
            Self::U8(v) => v.iter().map(|&x| f64::from(x)).collect(),
        }
    }

    fn from_ne_bytes(dtype: StreamDType, b: &[u8]) -> Self {
        match dtype {
            StreamDType::F64 => Self::F64(from_ne(b, f64::from_ne_bytes)),
            StreamDType::F32 => Self::F32(from_ne(b, f32::from_ne_bytes)),
            StreamDType::I32 => Self::I32(from_ne(b, i32::from_ne_bytes)),
            StreamDType::I64 => Self::I64(from_ne(b, i64::from_ne_bytes)),
            StreamDType::U8 => Self::U8(b.to_vec()),
        }
    }
}

/// One record's arrays: variable name → its row-major values over the
/// writer's region shape, without the time dim.
pub type RecordArrays<'a> = BTreeMap<String, TypedArray<'a>>;

// --- schema, roles, regions --------------------------------------------------

/// How the record axis is addressed (`spec/writer.md` §6).
#[derive(Debug, Clone, PartialEq)]
pub enum TimeMode {
    /// Records are appended; the index is the running count.
    Append,
    /// A fixed time axis of `n_records` records with coordinate `times`,
    /// written by index in any order.
    Indexed {
        /// Length of the time axis.
        n_records: usize,
        /// The time coordinate (`len == n_records`).
        times: Vec<f64>,
    },
}

/// A writer's role in a store (`spec/writer.md` §5).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Role {
    /// The only writer; `flush` commits.
    Single,
    /// Writer 0 of `n_writers`: owns every shared object and commits.
    Owner {
        /// Total number of writers, the owner included.
        n_writers: usize,
    },
    /// Writer `id` (`1..n_writers`): writes only its region.
    Contributor {
        /// This writer's id.
        id: usize,
    },
}

impl Role {
    fn id(self) -> usize {
        match self {
            Role::Single | Role::Owner { .. } => 0,
            Role::Contributor { id } => id,
        }
    }
}

/// A block of the non-time dims, `dim → start..stop` (0-based, half-open). An
/// omitted dim means its full extent; the empty region is the whole array.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Region(pub Vec<(String, Range<usize>)>);

impl Region {
    /// The whole array.
    pub fn whole() -> Self {
        Self(Vec::new())
    }

    /// A region from `(dim, start..stop)` pairs.
    pub fn new<I, D>(dims: I) -> Self
    where
        I: IntoIterator<Item = (D, Range<usize>)>,
        D: Into<String>,
    {
        Self(dims.into_iter().map(|(d, r)| (d.into(), r)).collect())
    }

    fn to_map(&self) -> Result<BTreeMap<String, Range<usize>>> {
        let mut m = BTreeMap::new();
        for (d, r) in &self.0 {
            if m.insert(d.clone(), r.clone()).is_some() {
                return Err(err(format!("region names dim '{d}' twice")));
            }
        }
        Ok(m)
    }
}

/// Where a blob belongs (`spec/writer.md` §7).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum BlobScope {
    /// Store-lifetime state: `blobs/static/<writer>/<key>`.
    Static,
    /// State of record `i`: `blobs/record/<i>/<writer>/<key>`.
    Record(u64),
}

impl BlobScope {
    fn wire(self) -> (&'static str, Option<u64>) {
        match self {
            BlobScope::Static => ("static", None),
            BlobScope::Record(i) => ("record", Some(i)),
        }
    }

    fn from_wire(scope: &str, record: Option<u64>) -> Result<Self> {
        match (scope, record) {
            ("static", None) => Ok(BlobScope::Static),
            ("record", Some(i)) => Ok(BlobScope::Record(i)),
            _ => Err(err(format!("bad blob scope '{scope}' / record {record:?}"))),
        }
    }

    fn key(self, writer: usize, key: &str) -> String {
        match self {
            BlobScope::Static => format!("blobs/static/{writer}/{key}"),
            BlobScope::Record(i) => format!("blobs/record/{i}/{writer}/{key}"),
        }
    }
}

/// One streaming variable (with a time dim) or static (without one).
#[derive(Debug, Clone)]
pub struct StreamVar {
    /// Variable name.
    pub name: String,
    /// Ordered dim names (file order).
    pub dims: Vec<String>,
    /// Element type.
    pub dtype: StreamDType,
    /// CF attributes for the array's `zarr.json`.
    pub attrs: Map<String, Value>,
}

/// The stream schema: the v1 [`OutputSchema`](super::OutputSchema) without
/// data, plus a time mode, statics and per-variable dtypes.
#[derive(Debug, Clone)]
pub struct StreamSchema {
    /// ORDERED dim name → length. The time dim's length is ignored in `Append`
    /// mode and replaced by `n_records` in `Indexed` mode.
    pub dims: Vec<(String, usize)>,
    /// The record axis.
    pub time_dim: String,
    /// dim name → inner chunk length.
    pub chunk_shape: BTreeMap<String, usize>,
    /// dim name → shard length (a multiple of the inner chunk length).
    pub shard_shape: BTreeMap<String, usize>,
    /// Static coordinates. An entry for `time_dim` supplies the time
    /// coordinate's attrs (its values are ignored) and makes the writer write a
    /// time coordinate at all.
    pub coords: Vec<WriteCoord>,
    /// Streaming variables (dims include `time_dim`).
    pub vars: Vec<StreamVar>,
    /// Time-less variables.
    pub statics: Vec<StreamVar>,
    /// `Append` or `Indexed`.
    pub time_mode: TimeMode,
    /// Group attributes.
    pub group_attrs: Map<String, Value>,
    /// Codec profile name (`diagnostic`, `checkpoint`, `wasm`).
    pub profile: String,
}

/// The value a writer hands the owner after `flush` (`spec/writer.md` §5.2).
/// Serializes to the documented JSON wire format.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CommitToken {
    /// Always [`COMMIT_TOKEN_SCHEMA`].
    pub schema: String,
    /// Writer id.
    pub writer: usize,
    /// The writer's region, `dim → [start, stop]`.
    pub region: BTreeMap<String, [usize; 2]>,
    /// The schema fingerprint.
    pub fingerprint: String,
    /// Records written since the previous token, ascending.
    pub records: Vec<TokenRecord>,
    /// Statics written since the previous token, sorted.
    pub statics: Vec<String>,
    /// Blobs written since the previous token, sorted.
    pub blobs: Vec<TokenBlob>,
    /// Store keys written since the previous token (advisory), sorted.
    pub objects: Vec<String>,
}

/// One record in a [`CommitToken`].
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TokenRecord {
    /// Record index.
    pub index: u64,
    /// Its time coordinate.
    pub t: f64,
}

/// One blob in a [`CommitToken`].
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub struct TokenBlob {
    /// `"static"` or `"record"`.
    pub scope: String,
    /// The record index for `"record"`, else `null`.
    pub record: Option<u64>,
    /// The blob key.
    pub key: String,
}

impl CommitToken {
    /// The JSON wire form.
    pub fn to_json(&self) -> String {
        serde_json::to_string(self).expect("a CommitToken always serializes")
    }

    /// Parse the JSON wire form.
    ///
    /// # Errors
    /// [`Error::Format`] on malformed JSON or a wrong `schema` label.
    pub fn from_json(s: &str) -> Result<Self> {
        let t: Self = serde_json::from_str(s).map_err(|e| err(format!("bad commit token: {e}")))?;
        if t.schema != COMMIT_TOKEN_SCHEMA {
            return Err(err(format!(
                "commit token schema '{}' != '{COMMIT_TOKEN_SCHEMA}'",
                t.schema
            )));
        }
        Ok(t)
    }

    /// True when the token carries no records, statics or blobs.
    pub fn is_empty(&self) -> bool {
        self.records.is_empty() && self.statics.is_empty() && self.blobs.is_empty()
    }
}

/// What a successful [`commit`](ZarrStreamWriter::commit) did.
#[derive(Debug, Clone, PartialEq)]
pub struct CommitReceipt {
    /// The manifest's `commit_seq` after this commit.
    pub commit_seq: u64,
    /// Record indices committed by this commit.
    pub records: Vec<u64>,
    /// Statics committed by this commit.
    pub statics: Vec<String>,
    /// `Append` mode: committed record count after this commit.
    pub n_committed: u64,
}

// --- canonical JSON + fingerprint (spec/writer.md §3.1) ----------------------

fn canonical_json(v: &Value, out: &mut String) {
    match v {
        Value::Null => out.push_str("null"),
        Value::Bool(b) => out.push_str(if *b { "true" } else { "false" }),
        Value::Number(n) => out.push_str(&n.to_string()),
        Value::String(s) => canonical_str(s, out),
        Value::Array(a) => {
            out.push('[');
            for (i, x) in a.iter().enumerate() {
                if i > 0 {
                    out.push(',');
                }
                canonical_json(x, out);
            }
            out.push(']');
        }
        Value::Object(m) => {
            let mut keys: Vec<&String> = m.keys().collect();
            keys.sort();
            out.push('{');
            for (i, k) in keys.into_iter().enumerate() {
                if i > 0 {
                    out.push(',');
                }
                canonical_str(k, out);
                out.push(':');
                canonical_json(&m[k], out);
            }
            out.push('}');
        }
    }
}

// Python's `json.dumps(..., ensure_ascii=True)` string escaping.
fn canonical_str(s: &str, out: &mut String) {
    out.push('"');
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            '\u{8}' => out.push_str("\\b"),
            '\u{c}' => out.push_str("\\f"),
            c if (c as u32) < 0x20 || (c as u32) > 0x7e && (c as u32) != 0x7f => {
                let mut buf = [0u16; 2];
                for u in c.encode_utf16(&mut buf) {
                    out.push_str(&format!("\\u{u:04x}"));
                }
            }
            c => out.push(c),
        }
    }
    out.push('"');
}

fn var_triples(vs: &[StreamVar]) -> Value {
    Value::Array(
        vs.iter()
            .map(|v| json!([v.name, v.dims, v.dtype.name()]))
            .collect(),
    )
}

/// The schema fingerprint (`spec/writer.md` §3.1): `"sha256:" + hex` over the
/// canonical JSON of the schema's structural fields.
pub fn schema_fingerprint(schema: &StreamSchema) -> String {
    let (mode, n) = match &schema.time_mode {
        TimeMode::Append => ("append", 0),
        TimeMode::Indexed { n_records, .. } => ("indexed", *n_records),
    };
    let dims: Vec<Value> = schema
        .dims
        .iter()
        .map(|(d, l)| json!([d, if *d == schema.time_dim { n } else { *l }]))
        .collect();
    let mut f = Map::new();
    f.insert("chunk_shape".into(), json!(schema.chunk_shape));
    f.insert("dims".into(), Value::Array(dims));
    f.insert("profile".into(), json!(schema.profile));
    f.insert("shard_shape".into(), json!(schema.shard_shape));
    f.insert("statics".into(), var_triples(&schema.statics));
    f.insert("time_dim".into(), json!(schema.time_dim));
    f.insert("time_mode".into(), json!(mode));
    f.insert("vars".into(), var_triples(&schema.vars));
    let mut s = String::new();
    canonical_json(&Value::Object(f), &mut s);
    let digest = Sha256::digest(s.as_bytes());
    let hex: String = digest.iter().map(|b| format!("{b:02x}")).collect();
    format!("sha256:{hex}")
}

// --- n-d byte boxes ----------------------------------------------------------

/// A dense row-major block of a larger array: `origin` is its global start per
/// dim, `shape` its extent, `data` its native-endian element bytes.
#[derive(Debug, Clone)]
struct NdBuf {
    origin: Vec<usize>,
    shape: Vec<usize>,
    esize: usize,
    data: Vec<u8>,
}

impl NdBuf {
    fn zeros(origin: Vec<usize>, shape: Vec<usize>, esize: usize) -> Self {
        let n: usize = shape.iter().product();
        Self {
            origin,
            shape,
            esize,
            data: vec![0u8; n * esize],
        }
    }
}

fn strides(shape: &[usize]) -> Vec<usize> {
    let mut s = vec![1usize; shape.len()];
    for d in (0..shape.len().saturating_sub(1)).rev() {
        s[d] = s[d + 1] * shape[d + 1];
    }
    s
}

/// Copy the intersection of two boxes from `src` into `dst`.
fn copy_overlap(src: &NdBuf, dst: &mut NdBuf) {
    let n = src.shape.len();
    debug_assert_eq!(n, dst.shape.len());
    let es = src.esize;
    if n == 0 {
        dst.data.copy_from_slice(&src.data);
        return;
    }
    let lo: Vec<usize> = (0..n).map(|d| src.origin[d].max(dst.origin[d])).collect();
    let hi: Vec<usize> = (0..n)
        .map(|d| (src.origin[d] + src.shape[d]).min(dst.origin[d] + dst.shape[d]))
        .collect();
    if (0..n).any(|d| lo[d] >= hi[d]) {
        return;
    }
    let (ss, ds) = (strides(&src.shape), strides(&dst.shape));
    let run = (hi[n - 1] - lo[n - 1]) * es;
    let mut cur = lo.clone();
    loop {
        let so: usize = (0..n)
            .map(|d| (cur[d] - src.origin[d]) * ss[d])
            .sum::<usize>()
            * es;
        let dof: usize = (0..n)
            .map(|d| (cur[d] - dst.origin[d]) * ds[d])
            .sum::<usize>()
            * es;
        dst.data[dof..dof + run].copy_from_slice(&src.data[so..so + run]);
        // odometer over every dim but the last
        let mut d = n - 1;
        loop {
            if d == 0 {
                return;
            }
            d -= 1;
            cur[d] += 1;
            if cur[d] < hi[d] {
                break;
            }
            cur[d] = lo[d];
        }
    }
}

/// Every index tuple of the product of `ranges` (row-major order).
fn product(ranges: &[Range<u64>]) -> Vec<Vec<u64>> {
    let mut out = vec![Vec::new()];
    for r in ranges {
        let mut next = Vec::new();
        for p in &out {
            for i in r.clone() {
                let mut q = p.clone();
                q.push(i);
                next.push(q);
            }
        }
        out = next;
    }
    out
}

// --- array metadata + chunk I/O ----------------------------------------------

#[allow(clippy::too_many_arguments)]
fn array_meta(
    dims: &[String],
    shape: &[u64],
    chunk: &BTreeMap<String, usize>,
    shard: &BTreeMap<String, usize>,
    dtype: StreamDType,
    codec: &CodecProfile,
    attrs: &Map<String, Value>,
) -> Result<Value> {
    let pick = |m: &BTreeMap<String, usize>, what: &str| -> Result<Vec<usize>> {
        dims.iter()
            .map(|d| {
                m.get(d)
                    .copied()
                    .ok_or_else(|| err(format!("dim '{d}' missing from {what}")))
            })
            .collect()
    };
    let inner = pick(chunk, "chunk_shape")?;
    let outer = pick(shard, "shard_shape")?;
    let mut a = attrs.clone();
    a.insert("_ARRAY_DIMENSIONS".to_string(), json!(dims));
    let fill = if dtype.is_float() {
        json!(0.0)
    } else {
        json!(0)
    };
    Ok(json!({
        "zarr_format": 3,
        "node_type": "array",
        "shape": shape,
        "data_type": dtype.name(),
        "chunk_grid": {"name": "regular", "configuration": {"chunk_shape": outer}},
        "chunk_key_encoding": {"name": "default", "configuration": {"separator": "/"}},
        "fill_value": fill,
        "codecs": [sharding_codec(&inner, codec, dtype.size())],
        "attributes": Value::Object(a),
        "dimension_names": dims,
    }))
}

fn array_from_meta<S>(store: &Arc<S>, name: &str, meta: &Value) -> Result<Array<S>>
where
    S: ReadableWritableStorageTraits + 'static,
{
    let md: ArrayMetadata = serde_json::from_value(meta.clone())
        .map_err(|e| err(format!("array '{name}' metadata is not valid v3: {e}")))?;
    Array::new_with_metadata(store.clone(), &format!("/{name}"), md)
        .map_err(|e| err(format!("build zarr array '{name}': {e}")))
}

fn store_chunk_bytes<S>(array: &Array<S>, idx: &[u64], dtype: StreamDType, b: &[u8]) -> Result<()>
where
    S: ReadableWritableStorageTraits + 'static,
{
    let r = match TypedVec::from_ne_bytes(dtype, b) {
        TypedVec::F64(v) => array.store_chunk(idx, v),
        TypedVec::F32(v) => array.store_chunk(idx, v),
        TypedVec::I32(v) => array.store_chunk(idx, v),
        TypedVec::I64(v) => array.store_chunk(idx, v),
        TypedVec::U8(v) => array.store_chunk(idx, v),
    };
    r.map_err(|e| err(format!("write shard {idx:?}: {e}")))
}

fn retrieve<S>(array: &Array<S>, subset: &ArraySubset, dtype: StreamDType) -> Result<TypedVec>
where
    S: ReadableStorageTraits + ?Sized + 'static,
{
    let e = |e: zarrs::array::ArrayError| err(format!("read {subset:?}: {e}"));
    Ok(match dtype {
        StreamDType::F64 => {
            TypedVec::F64(array.retrieve_array_subset::<Vec<f64>>(subset).map_err(e)?)
        }
        StreamDType::F32 => {
            TypedVec::F32(array.retrieve_array_subset::<Vec<f32>>(subset).map_err(e)?)
        }
        StreamDType::I32 => {
            TypedVec::I32(array.retrieve_array_subset::<Vec<i32>>(subset).map_err(e)?)
        }
        StreamDType::I64 => {
            TypedVec::I64(array.retrieve_array_subset::<Vec<i64>>(subset).map_err(e)?)
        }
        StreamDType::U8 => TypedVec::U8(array.retrieve_array_subset::<Vec<u8>>(subset).map_err(e)?),
    })
}

fn get_json<S: ReadableStorageTraits + ?Sized>(store: &S, key: &str) -> Result<Option<Value>> {
    let k = StoreKey::new(key).map_err(|e| err(format!("invalid store key '{key}': {e}")))?;
    match store.get(&k).map_err(|e| err(format!("read {key}: {e}")))? {
        None => Ok(None),
        Some(b) => serde_json::from_slice(&b)
            .map(Some)
            .map_err(|e| err(format!("parse {key}: {e}"))),
    }
}

fn codec_meta(codec: &CodecProfile) -> Value {
    match codec {
        CodecProfile::Blosc(b) => json!({
            "id": "blosc",
            "cname": b.cname,
            "clevel": b.clevel,
            "shuffle": if b.shuffle { "shuffle" } else { "noshuffle" },
        }),
        CodecProfile::Zstd(z) => json!({
            "id": "zstd",
            "level": z.level,
            "checksum": z.checksum,
        }),
    }
}

// --- the schema-derived geometry ---------------------------------------------

/// Everything a write needs that does not change after open.
#[derive(Debug, Clone)]
struct Geometry {
    schema: StreamSchema,
    codec: CodecProfile,
    region: BTreeMap<String, Range<usize>>,
    dim_len: BTreeMap<String, usize>,
    st: u64,
}

impl Geometry {
    fn n_indexed(&self) -> Option<u64> {
        match &self.schema.time_mode {
            TimeMode::Append => None,
            TimeMode::Indexed { n_records, .. } => Some(*n_records as u64),
        }
    }

    fn is_time(&self, d: &str) -> bool {
        d == self.schema.time_dim
    }

    fn partitioned(&self, dims: &[String]) -> bool {
        dims.iter().any(|d| self.region.contains_key(d))
    }

    /// Whether this writer owns the shard objects of an array over `dims`: a
    /// partitioned array that lacks some region dim (`[time, lon]` under a
    /// `{lat, lon}` split) is supplied by every writer but written only by the
    /// one whose region starts at 0 on each missing dim, so every shard object
    /// keeps exactly one writer (`spec/writer.md` §4).
    fn owns(&self, dims: &[String]) -> bool {
        self.region
            .iter()
            .all(|(d, r)| dims.contains(d) || r.start == 0)
    }

    fn local(&self, d: &str) -> Range<usize> {
        self.region.get(d).cloned().unwrap_or(0..self.dim_len[d])
    }

    /// The writer's box over `dims` for the record slab `[t0, t0+tn)`.
    fn var_box(&self, dims: &[String], t0: usize, tn: usize) -> (Vec<usize>, Vec<usize>) {
        dims.iter()
            .map(|d| {
                if self.is_time(d) {
                    (t0, tn)
                } else {
                    let r = self.local(d);
                    (r.start, r.end - r.start)
                }
            })
            .unzip()
    }

    fn local_len(&self, dims: &[String]) -> usize {
        dims.iter()
            .filter(|d| !self.is_time(d))
            .map(|d| self.local(d).len())
            .product()
    }

    /// An in-memory array handle whose time extent is `time_extent` (never
    /// stored: only the owner's committed shapes reach `zarr.json`).
    fn handle<S>(
        &self,
        store: &Arc<S>,
        name: &str,
        dims: &[String],
        dtype: StreamDType,
        time_extent: u64,
    ) -> Result<Array<S>>
    where
        S: ReadableWritableStorageTraits + 'static,
    {
        let shape: Vec<u64> = dims
            .iter()
            .map(|d| {
                if self.is_time(d) {
                    time_extent
                } else {
                    self.dim_len[d] as u64
                }
            })
            .collect();
        let meta = array_meta(
            dims,
            &shape,
            &self.schema.chunk_shape,
            &self.schema.shard_shape,
            dtype,
            &self.codec,
            &Map::new(),
        )?;
        array_from_meta(store, name, &meta)
    }

    /// Encode and write every shard object `buf` covers. Returns the keys.
    fn write_box<S>(
        &self,
        store: &Arc<S>,
        name: &str,
        dims: &[String],
        dtype: StreamDType,
        buf: &NdBuf,
        time_extent: u64,
    ) -> Result<Vec<String>>
    where
        S: ReadableWritableStorageTraits + 'static,
    {
        let array = self.handle(store, name, dims, dtype, time_extent)?;
        let sz: Vec<usize> = dims.iter().map(|d| self.schema.shard_shape[d]).collect();
        let ranges: Vec<Range<u64>> = (0..dims.len())
            .map(|k| {
                let (o, s) = (buf.origin[k], buf.shape[k]);
                (o / sz[k]) as u64..(o + s).div_ceil(sz[k]) as u64
            })
            .collect();
        let mut keys = Vec::new();
        for idx in product(&ranges) {
            let origin: Vec<usize> = idx.iter().zip(&sz).map(|(&i, &s)| i as usize * s).collect();
            let mut chunk = NdBuf::zeros(origin, sz.clone(), dtype.size());
            copy_overlap(buf, &mut chunk);
            store_chunk_bytes(&array, &idx, dtype, &chunk.data)?;
            let tail: Vec<String> = idx.iter().map(u64::to_string).collect();
            keys.push(if tail.is_empty() {
                format!("{name}/c")
            } else {
                format!("{name}/c/{}", tail.join("/"))
            });
        }
        Ok(keys)
    }

    /// Fill `buf` from the store, reading only time indices below `time_limit`.
    fn reload_box<S>(
        &self,
        store: &Arc<S>,
        name: &str,
        dims: &[String],
        dtype: StreamDType,
        buf: &mut NdBuf,
        time_limit: u64,
    ) -> Result<()>
    where
        S: ReadableWritableStorageTraits + 'static,
    {
        let ranges: Vec<Range<u64>> = dims
            .iter()
            .enumerate()
            .map(|(k, d)| {
                let (o, s) = (buf.origin[k] as u64, buf.shape[k] as u64);
                if self.is_time(d) {
                    o..(o + s).min(time_limit).max(o)
                } else {
                    o..o + s
                }
            })
            .collect();
        if ranges.iter().any(|r| r.is_empty()) {
            return Ok(());
        }
        let array = self.handle(store, name, dims, dtype, time_limit)?;
        let subset = ArraySubset::new_with_ranges(&ranges);
        let got = retrieve(&array, &subset, dtype)?;
        let src = NdBuf {
            origin: ranges.iter().map(|r| r.start as usize).collect(),
            shape: ranges.iter().map(|r| (r.end - r.start) as usize).collect(),
            esize: dtype.size(),
            data: got.as_array().to_ne_bytes(),
        };
        copy_overlap(&src, buf);
        Ok(())
    }
}

fn validate_schema(schema: &mut StreamSchema) -> Result<()> {
    let _ = profile(&schema.profile)?;
    let td = schema.time_dim.clone();
    if !schema.dims.iter().any(|(d, _)| *d == td) {
        return Err(err(format!("time dim '{td}' is not in dims")));
    }
    if let TimeMode::Indexed { n_records, times } = &schema.time_mode {
        if times.len() != *n_records {
            return Err(err(format!(
                "indexed: {} times for n_records = {n_records}",
                times.len()
            )));
        }
        let n = *n_records;
        for (d, l) in &mut schema.dims {
            if *d == td {
                *l = n;
            }
        }
    }
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
    let known: BTreeSet<&str> = schema.dims.iter().map(|(d, _)| d.as_str()).collect();
    let mut names = BTreeSet::new();
    for v in schema.vars.iter().chain(&schema.statics) {
        if !names.insert(v.name.as_str()) || v.name == td {
            return Err(err(format!("duplicate variable name '{}'", v.name)));
        }
        if let Some(d) = v.dims.iter().find(|d| !known.contains(d.as_str())) {
            return Err(err(format!(
                "var '{}' dim '{d}' not in schema dims",
                v.name
            )));
        }
    }
    for v in &schema.vars {
        if v.dims.iter().filter(|d| **d == td).count() != 1 {
            return Err(err(format!(
                "streaming var '{}' must include the time dim '{td}'",
                v.name
            )));
        }
    }
    for v in &schema.statics {
        if v.dims.contains(&td) {
            return Err(err(format!(
                "static '{}' must not include the time dim '{td}'",
                v.name
            )));
        }
    }
    Ok(())
}

fn validate_region(schema: &StreamSchema, region: &BTreeMap<String, Range<usize>>) -> Result<()> {
    let len: BTreeMap<&str, usize> = schema.dims.iter().map(|(d, l)| (d.as_str(), *l)).collect();
    for (d, r) in region {
        let bad = |detail: String| Error::RegionNotShardAligned { detail };
        if *d == schema.time_dim {
            return Err(bad(format!("a region may not name the time dim '{d}'")));
        }
        let n = *len
            .get(d.as_str())
            .ok_or_else(|| bad(format!("region dim '{d}' is not a schema dim")))?;
        let s = schema.shard_shape[d];
        if r.start >= r.end || r.end > n {
            return Err(bad(format!(
                "{d}: [{}, {}) is not within [0, {n})",
                r.start, r.end
            )));
        }
        if r.start % s != 0 {
            return Err(bad(format!(
                "{d}: start {} is not a multiple of the shard length {s}",
                r.start
            )));
        }
        if r.end % s != 0 && r.end != n {
            return Err(bad(format!(
                "{d}: stop {} is neither a multiple of the shard length {s} nor the dim length {n}",
                r.end
            )));
        }
    }
    Ok(())
}

fn wire_region(region: &BTreeMap<String, Range<usize>>) -> BTreeMap<String, [usize; 2]> {
    region
        .iter()
        .map(|(d, r)| (d.clone(), [r.start, r.end]))
        .collect()
}

// --- committed state ----------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Version {
    V1,
    V2,
}

#[derive(Debug, Clone)]
struct TimeShardRec {
    index: u64,
    t_start: f64,
    t_end: f64,
    n: u64,
}

#[derive(Debug, Clone)]
struct Entry {
    t: f64,
    committed: bool,
    generation: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
struct BlobRec {
    scope: BlobScope,
    writer: usize,
    key: String,
}

/// The committed state the manifest describes: authoritative on the owner, a
/// view (refreshed from the manifest when it matters) on a contributor.
#[derive(Debug, Clone, Default)]
struct CommitState {
    commit_seq: u64,
    n_committed: u64,
    time_shards: Vec<TimeShardRec>,
    last_t: Option<f64>,
    entries: Vec<Entry>,
    statics: BTreeMap<String, (bool, u64)>,
    blobs: BTreeSet<BlobRec>,
    writers: Vec<(usize, BTreeMap<String, [usize; 2]>)>,
}

impl CommitState {
    fn rebuild_append_shards(&mut self, ts: &[f64], st: u64) {
        self.time_shards.clear();
        for (i, &t) in ts.iter().enumerate() {
            self.push_append(i as u64, t, st);
        }
        self.n_committed = ts.len() as u64;
        self.last_t = ts.last().copied();
    }

    fn push_append(&mut self, index: u64, t: f64, st: u64) {
        let s = index / st;
        match self.time_shards.last_mut() {
            Some(last) if last.index == s => {
                last.t_end = t;
                last.n += 1;
            }
            _ => self.time_shards.push(TimeShardRec {
                index: s,
                t_start: t,
                t_end: t,
                n: 1,
            }),
        }
        self.last_t = Some(t);
    }

    fn rebuild_indexed_shards(&mut self, st: u64) {
        self.time_shards.clear();
        self.last_t = None;
        for (i, e) in self.entries.iter().enumerate() {
            if !e.committed {
                continue;
            }
            let s = i as u64 / st;
            match self.time_shards.last_mut() {
                Some(last) if last.index == s => {
                    last.t_start = last.t_start.min(e.t);
                    last.t_end = last.t_end.max(e.t);
                    last.n += 1;
                }
                _ => self.time_shards.push(TimeShardRec {
                    index: s,
                    t_start: e.t,
                    t_end: e.t,
                    n: 1,
                }),
            }
            self.last_t = Some(self.last_t.map_or(e.t, |m: f64| m.max(e.t)));
        }
    }
}

fn committed_ranges(state: &CommitState, indexed: bool) -> Vec<[u64; 2]> {
    if !indexed {
        return if state.n_committed > 0 {
            vec![[0, state.n_committed]]
        } else {
            vec![]
        };
    }
    let mut out: Vec<[u64; 2]> = Vec::new();
    for (i, e) in state.entries.iter().enumerate() {
        if !e.committed {
            continue;
        }
        let i = i as u64;
        match out.last_mut() {
            Some(r) if r[1] == i => r[1] = i + 1,
            _ => out.push([i, i + 1]),
        }
    }
    out
}

/// Rebuild the stream schema, version and committed state from a manifest.
fn from_manifest(m: &Value) -> Result<(StreamSchema, Version, CommitState)> {
    let bad = |w: &str| err(format!("output manifest: bad or missing '{w}'"));
    let version = match m["schema"].as_str() {
        Some(OUTPUT_MANIFEST_V1) => Version::V1,
        Some(OUTPUT_MANIFEST_V2) => Version::V2,
        other => return Err(err(format!("unknown output manifest schema {other:?}"))),
    };
    let s = |v: &Value, w: &str| v.as_str().map(str::to_string).ok_or_else(|| bad(w));
    let u = |v: &Value, w: &str| v.as_u64().ok_or_else(|| bad(w));
    let time_dim = s(&m["time_dim"], "time_dim")?;
    let dims: Vec<(String, usize)> = m["dims"]
        .as_array()
        .ok_or_else(|| bad("dims"))?
        .iter()
        .map(|d| {
            Ok((
                s(&d["name"], "dims.name")?,
                u(&d["length"], "dims.length")? as usize,
            ))
        })
        .collect::<Result<_>>()?;
    let umap = |v: &Value, w: &str| -> Result<BTreeMap<String, usize>> {
        v.as_object()
            .ok_or_else(|| bad(w))?
            .iter()
            .map(|(k, x)| Ok((k.clone(), x.as_u64().ok_or_else(|| bad(w))? as usize)))
            .collect()
    };
    let parse_vars = |v: &Value, w: &str| -> Result<Vec<StreamVar>> {
        v.as_array()
            .map(|a| a.as_slice())
            .unwrap_or(&[])
            .iter()
            .map(|e| {
                Ok(StreamVar {
                    name: s(&e["name"], w)?,
                    dims: e["dims"]
                        .as_array()
                        .ok_or_else(|| bad(w))?
                        .iter()
                        .map(|d| s(d, w))
                        .collect::<Result<_>>()?,
                    dtype: StreamDType::parse(e["dtype"].as_str().ok_or_else(|| bad(w))?)?,
                    attrs: e["attrs"].as_object().cloned().unwrap_or_default(),
                })
            })
            .collect()
    };
    let mut state = CommitState::default();
    let n_records = u(&m["n_records"], "n_records")?;
    let mut time_mode = TimeMode::Append;
    if version == Version::V2 {
        state.commit_seq = u(&m["commit_seq"], "commit_seq")?;
        if m["time_mode"]["kind"] == "indexed" {
            let entries = m["records"]["entries"]
                .as_array()
                .ok_or_else(|| bad("records.entries"))?;
            for e in entries {
                state.entries.push(Entry {
                    t: e["t"].as_f64().ok_or_else(|| bad("entries.t"))?,
                    committed: e["committed"]
                        .as_bool()
                        .ok_or_else(|| bad("entries.committed"))?,
                    generation: u(&e["generation"], "entries.generation")?,
                });
            }
            time_mode = TimeMode::Indexed {
                n_records: entries.len(),
                times: state.entries.iter().map(|e| e.t).collect(),
            };
        }
        for e in m["statics"].as_array().map(|a| a.as_slice()).unwrap_or(&[]) {
            state.statics.insert(
                s(&e["name"], "statics.name")?,
                (
                    e["committed"].as_bool().unwrap_or(false),
                    e["generation"].as_u64().unwrap_or(0),
                ),
            );
        }
        for b in m["blobs"].as_array().map(|a| a.as_slice()).unwrap_or(&[]) {
            state.blobs.insert(BlobRec {
                scope: BlobScope::from_wire(&s(&b["scope"], "blobs.scope")?, b["record"].as_u64())?,
                writer: u(&b["writer"], "blobs.writer")? as usize,
                key: s(&b["key"], "blobs.key")?,
            });
        }
        for w in m["writers"].as_array().map(|a| a.as_slice()).unwrap_or(&[]) {
            let region: BTreeMap<String, [usize; 2]> =
                serde_json::from_value(w["region"].clone()).map_err(|_| bad("writers.region"))?;
            state
                .writers
                .push((u(&w["id"], "writers.id")? as usize, region));
        }
    }
    if matches!(time_mode, TimeMode::Append) {
        state.n_committed = n_records;
    }
    for t in m["time_shards"]
        .as_array()
        .ok_or_else(|| bad("time_shards"))?
    {
        state.time_shards.push(TimeShardRec {
            index: u(&t["index"], "time_shards.index")?,
            t_start: t["t_start"]
                .as_f64()
                .ok_or_else(|| bad("time_shards.t_start"))?,
            t_end: t["t_end"]
                .as_f64()
                .ok_or_else(|| bad("time_shards.t_end"))?,
            n: u(&t["n"], "time_shards.n")?,
        });
    }
    state.last_t = m["last_t"].as_f64();
    let schema = StreamSchema {
        dims,
        time_dim,
        chunk_shape: umap(&m["chunk_shape"], "chunk_shape")?,
        shard_shape: umap(&m["shard_shape"], "shard_shape")?,
        coords: Vec::new(),
        vars: parse_vars(&m["vars"], "vars")?,
        statics: parse_vars(&m["statics"], "statics")?,
        time_mode,
        group_attrs: Map::new(),
        profile: s(&m["profile"], "profile")?,
    };
    Ok((schema, version, state))
}

// --- the writer ----------------------------------------------------------------

#[derive(Debug, Default)]
struct Pending {
    records: BTreeMap<u64, f64>,
    statics: BTreeSet<String>,
    blobs: BTreeSet<(BlobScope, String)>,
    objects: BTreeSet<String>,
}

impl Pending {
    fn has_content(&self) -> bool {
        !(self.records.is_empty() && self.statics.is_empty() && self.blobs.is_empty())
    }
}

/// One open time shard: the writer's box of every array it writes, plus which
/// record slots it has filled.
#[derive(Debug)]
struct ShardBuf {
    bufs: BTreeMap<String, NdBuf>,
    written: Vec<bool>,
}

/// The streaming writer handle (`spec/writer.md`). See the module docs.
pub struct ZarrStreamWriter<S> {
    store: Arc<S>,
    base_url: String,
    geo: Geometry,
    role: Role,
    fingerprint: String,
    version: Version,
    time_coord: bool,
    state: CommitState,
    next_index: u64,
    last_written_t: Option<f64>,
    open: BTreeMap<u64, ShardBuf>,
    dirty: BTreeSet<u64>,
    /// Shards this handle has written (indexed mode: reload when reopened).
    flushed: BTreeSet<u64>,
    /// False for a store this handle created: no shard object can pre-exist.
    maybe_on_disk: bool,
    pending: Pending,
    tokened_records: BTreeSet<u64>,
    tokened_statics: BTreeSet<String>,
    meta: BTreeMap<String, Value>,
}

impl<S> std::fmt::Debug for ZarrStreamWriter<S> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ZarrStreamWriter")
            .field("base_url", &self.base_url)
            .field("role", &self.role)
            .field("region", &self.geo.region)
            .field("next_index", &self.next_index)
            .finish_non_exhaustive()
    }
}

fn check_role(role: Role) -> Result<()> {
    if cfg!(target_arch = "wasm32") && role != Role::Single {
        return Err(err("only Role::Single is supported on wasm32"));
    }
    match role {
        Role::Owner { n_writers: 0 } => Err(err("Owner { n_writers } must be >= 1")),
        Role::Contributor { id: 0 } => {
            Err(err("writer id 0 is the owner; a contributor id is >= 1"))
        }
        _ => Ok(()),
    }
}

impl<S> ZarrStreamWriter<S>
where
    S: ReadableWritableStorageTraits + 'static,
{
    fn assemble(
        store: Arc<S>,
        base_url: &str,
        schema: StreamSchema,
        role: Role,
        region: BTreeMap<String, Range<usize>>,
        version: Version,
        state: CommitState,
    ) -> Result<Self> {
        let codec = profile(&schema.profile)?;
        let dim_len = schema.dims.iter().cloned().collect();
        let st = schema.shard_shape[&schema.time_dim] as u64;
        let fingerprint = schema_fingerprint(&schema);
        let time_coord = schema.coords.iter().any(|c| c.name == schema.time_dim);
        Ok(Self {
            store,
            base_url: base_url.to_string(),
            geo: Geometry {
                schema,
                codec,
                region,
                dim_len,
                st,
            },
            role,
            fingerprint,
            version,
            time_coord,
            next_index: state.n_committed,
            last_written_t: state.last_t,
            state,
            open: BTreeMap::new(),
            dirty: BTreeSet::new(),
            flushed: BTreeSet::new(),
            maybe_on_disk: true,
            pending: Pending::default(),
            tokened_records: BTreeSet::new(),
            tokened_statics: BTreeSet::new(),
            meta: BTreeMap::new(),
        })
    }

    /// Create a store: the owner (or the single writer) writes the group, the
    /// coordinates, every array's `zarr.json` and, for a v2 store, the initial
    /// manifest (`commit_seq` 0).
    ///
    /// # Errors
    /// [`Error::RegionNotShardAligned`] for a misaligned region (before any
    /// I/O); [`Error::Format`] for a schema inconsistency, a contributor role
    /// (use [`join`](Self::join)), or a store failure.
    pub fn create(
        store: Arc<S>,
        base_url: &str,
        mut schema: StreamSchema,
        role: Role,
        region: Region,
    ) -> Result<Self> {
        check_role(role)?;
        validate_schema(&mut schema)?;
        let region = region.to_map()?;
        validate_region(&schema, &region)?;
        match role {
            Role::Contributor { .. } => {
                return Err(err("a contributor opens a store with join, not create"))
            }
            Role::Single if !region.is_empty() => return Err(err(
                "a Single writer writes the whole array; use Owner { n_writers: 1 } for a region",
            )),
            _ => {}
        }
        let version = if role == Role::Single
            && schema.time_mode == TimeMode::Append
            && schema.statics.is_empty()
        {
            Version::V1
        } else {
            Version::V2
        };
        let mut state = CommitState::default();
        if let TimeMode::Indexed { times, .. } = &schema.time_mode {
            state.entries = times
                .iter()
                .map(|&t| Entry {
                    t,
                    committed: false,
                    generation: 0,
                })
                .collect();
        }
        for s in &schema.statics {
            state.statics.insert(s.name.clone(), (false, 0));
        }
        state.writers = vec![(0, wire_region(&region))];
        let mut w = Self::assemble(store, base_url, schema, role, region, version, state)?;
        w.maybe_on_disk = false;
        w.write_layout()?;
        if w.version == Version::V2 {
            w.write_manifest()?;
        }
        Ok(w)
    }

    fn write_layout(&mut self) -> Result<()> {
        let schema = self.geo.schema.clone();
        let codec = self.geo.codec;
        let group = json!({
            "zarr_format": 3,
            "node_type": "group",
            "attributes": Value::Object(schema.group_attrs.clone()),
        });
        set_json(self.store.as_ref(), "zarr.json", &group)?;
        let td = schema.time_dim.clone();
        let n_time = self.geo.n_indexed().unwrap_or(0);
        let mut time_attrs = Map::new();
        for co in &schema.coords {
            if co.name == td {
                time_attrs = co.attrs.clone();
                continue;
            }
            let meta = array_meta(
                std::slice::from_ref(&co.name),
                &[co.values.len() as u64],
                &schema.chunk_shape,
                &schema.shard_shape,
                StreamDType::F64,
                &codec,
                &co.attrs,
            )?;
            let array = array_from_meta(&self.store, &co.name, &meta)?;
            array
                .store_metadata()
                .map_err(|e| err(format!("write metadata for '{}': {e}", co.name)))?;
            array
                .store_array_subset(&array.subset_all(), co.values.as_slice())
                .map_err(|e| err(format!("write data for '{}': {e}", co.name)))?;
        }
        if self.time_coord {
            let dims = vec![td.clone()];
            self.put_meta(&td, &dims, &[n_time], StreamDType::F64, &time_attrs)?;
            if let TimeMode::Indexed { times, .. } = &schema.time_mode {
                let buf = NdBuf {
                    origin: vec![0],
                    shape: vec![times.len()],
                    esize: 8,
                    data: TypedArray::F64(times).to_ne_bytes(),
                };
                self.geo
                    .write_box(&self.store, &td, &dims, StreamDType::F64, &buf, n_time)?;
            }
        }
        for v in &schema.vars {
            let shape: Vec<u64> = v
                .dims
                .iter()
                .map(|d| {
                    if *d == td {
                        n_time
                    } else {
                        self.geo.dim_len[d] as u64
                    }
                })
                .collect();
            self.put_meta(&v.name, &v.dims, &shape, v.dtype, &v.attrs)?;
        }
        for v in &schema.statics {
            let shape: Vec<u64> = v.dims.iter().map(|d| self.geo.dim_len[d] as u64).collect();
            self.put_meta(&v.name, &v.dims, &shape, v.dtype, &v.attrs)?;
        }
        Ok(())
    }

    fn put_meta(
        &mut self,
        name: &str,
        dims: &[String],
        shape: &[u64],
        dtype: StreamDType,
        attrs: &Map<String, Value>,
    ) -> Result<()> {
        let s = &self.geo.schema;
        let meta = array_meta(
            dims,
            shape,
            &s.chunk_shape,
            &s.shard_shape,
            dtype,
            &self.geo.codec,
            attrs,
        )?;
        self.store_meta(name, meta)
    }

    fn store_meta(&mut self, name: &str, meta: Value) -> Result<()> {
        array_from_meta(&self.store, name, &meta)?
            .store_metadata()
            .map_err(|e| err(format!("write metadata for '{name}': {e}")))?;
        self.meta.insert(name.to_string(), meta);
        Ok(())
    }

    /// Owner only: set `shape[time]` of every growable array to `n`.
    fn write_shapes(&mut self, n: u64) -> Result<()> {
        let td = self.geo.schema.time_dim.clone();
        let mut names: Vec<String> = self
            .geo
            .schema
            .vars
            .iter()
            .map(|v| v.name.clone())
            .collect();
        if self.time_coord {
            names.insert(0, td.clone());
        }
        for name in names {
            let mut meta = match self.meta.get(&name) {
                Some(m) => m.clone(),
                None => get_json(self.store.as_ref(), &format!("{name}/zarr.json"))?
                    .ok_or_else(|| err(format!("array '{name}' has no zarr.json")))?,
            };
            let dims: Vec<String> = serde_json::from_value(meta["dimension_names"].clone())
                .map_err(|_| err(format!("array '{name}' has no dimension_names")))?;
            let ti = dims
                .iter()
                .position(|d| *d == td)
                .ok_or_else(|| err(format!("array '{name}' has no time dim")))?;
            meta["shape"][ti] = json!(n);
            self.store_meta(&name, meta)?;
        }
        Ok(())
    }

    /// Join a store the owner created, as a [`Role::Contributor`]: read the v2
    /// manifest, rebuild and fingerprint-check the schema, validate the region.
    ///
    /// # Errors
    /// [`Error::RegionNotShardAligned`]; [`Error::Format`] for a missing or v1
    /// manifest, a fingerprint mismatch, or a non-contributor role.
    pub fn join(store: Arc<S>, base_url: &str, role: Role, region: Region) -> Result<Self> {
        check_role(role)?;
        if !matches!(role, Role::Contributor { .. }) {
            return Err(err(
                "join is for contributors; the owner opens with create or resume",
            ));
        }
        let region = region.to_map()?;
        let (schema, version, state) = Self::open_manifest(store.as_ref())?;
        if version != Version::V2 {
            return Err(err("join needs a v2 store (created by an Owner)"));
        }
        validate_region(&schema, &region)?;
        let mut w = Self::assemble(store, base_url, schema, role, region, version, state)?;
        w.time_coord = w.has_array(&w.geo.schema.time_dim.clone())?;
        // Joining mid-shard (after a commit that ended inside a time shard).
        w.reload_open_append(w.state.n_committed)?;
        Ok(w)
    }

    fn open_manifest(store: &S) -> Result<(StreamSchema, Version, CommitState)> {
        let m = read_output_manifest(store)?;
        let (schema, version, state) = from_manifest(&m)?;
        if version == Version::V2 {
            let fp = schema_fingerprint(&schema);
            if m["schema_fingerprint"].as_str() != Some(fp.as_str()) {
                return Err(err(format!(
                    "schema fingerprint mismatch: manifest {} vs rebuilt {fp}",
                    m["schema_fingerprint"]
                )));
            }
        }
        Ok((schema, version, state))
    }

    fn has_array(&self, name: &str) -> Result<bool> {
        Ok(get_json(self.store.as_ref(), &format!("{name}/zarr.json"))?.is_some())
    }

    /// Reopen a store after preemption (`spec/writer.md` §7). In `Append` mode
    /// the committed records are truncated to those with `t <= at` (by the
    /// owner / single writer; every writer computes the same count), and each
    /// writer reloads its region of the open time shard. In `Indexed` mode
    /// `at` must be `None`.
    ///
    /// # Errors
    /// As [`join`](Self::join); also when `at` is given without a time
    /// coordinate to compare it against.
    pub fn resume(
        store: Arc<S>,
        base_url: &str,
        role: Role,
        region: Region,
        at: Option<f64>,
    ) -> Result<Self> {
        check_role(role)?;
        let region = region.to_map()?;
        let (schema, version, state) = Self::open_manifest(store.as_ref())?;
        if version == Version::V1 && role != Role::Single {
            return Err(err(
                "a v1 store has a single writer; resume it with Role::Single",
            ));
        }
        if role == Role::Single && !region.is_empty() {
            return Err(err("a Single writer writes the whole array"));
        }
        validate_region(&schema, &region)?;
        let mut w = Self::assemble(store, base_url, schema, role, region, version, state)?;
        let td = w.geo.schema.time_dim.clone();
        w.time_coord = w.has_array(&td)?;
        if w.geo.n_indexed().is_some() {
            if at.is_some() {
                return Err(err("resume `at` applies to append mode only"));
            }
            return Ok(w);
        }
        let n = w.state.n_committed;
        let mut n2 = n;
        if let Some(at) = at {
            if !w.time_coord {
                return Err(err(format!(
                    "resume at t = {at} needs the time coordinate '{td}'"
                )));
            }
            let tv = w.read_time_values(n)?;
            n2 = tv.iter().take_while(|&&t| t <= at).count() as u64;
            if n2 < n {
                let owner = !matches!(role, Role::Contributor { .. });
                w.state.rebuild_append_shards(&tv[..n2 as usize], w.geo.st);
                if owner {
                    w.write_shapes(n2)?;
                    if w.version == Version::V2 {
                        w.state.commit_seq += 1;
                    }
                    w.write_manifest()?;
                }
            }
        }
        w.next_index = n2;
        w.last_written_t = w.state.last_t;
        w.reload_open_append(n2)?;
        Ok(w)
    }

    /// Append mode, `n` committed records: load this writer's region of the
    /// partly committed time shard, so that its next write of that shard
    /// rewrites the committed records it holds rather than zeroing them.
    fn reload_open_append(&mut self, n: u64) -> Result<()> {
        let st = self.geo.st;
        if self.geo.n_indexed().is_some() || n % st == 0 {
            return Ok(());
        }
        let ts = n / st;
        let mut sb = self.new_shard_buf(ts);
        for (name, buf) in &mut sb.bufs {
            let (dims, dtype) = self.array_spec(name);
            self.geo
                .reload_box(&self.store, name, &dims, dtype, buf, n)?;
        }
        for slot in 0..(n - ts * st) as usize {
            sb.written[slot] = true;
        }
        self.open.insert(ts, sb);
        Ok(())
    }

    fn read_time_values(&self, n: u64) -> Result<Vec<f64>> {
        if n == 0 {
            return Ok(Vec::new());
        }
        let td = &self.geo.schema.time_dim;
        let dims = vec![td.clone()];
        let array = self
            .geo
            .handle(&self.store, td, &dims, StreamDType::F64, n)?;
        Ok(retrieve(
            &array,
            &ArraySubset::new_with_ranges(std::slice::from_ref(&(0..n))),
            StreamDType::F64,
        )?
        .to_f64())
    }

    // --- what this writer writes -------------------------------------------

    fn is_contributor(&self) -> bool {
        matches!(self.role, Role::Contributor { .. })
    }

    fn writes_time_coord(&self) -> bool {
        self.time_coord && !self.is_contributor() && self.geo.n_indexed().is_none()
    }

    fn my_vars(&self) -> Vec<&StreamVar> {
        self.geo
            .schema
            .vars
            .iter()
            .filter(|v| !self.is_contributor() || self.geo.partitioned(&v.dims))
            .collect()
    }

    /// dims + dtype of a buffered array (a variable or the time coordinate).
    fn array_spec(&self, name: &str) -> (Vec<String>, StreamDType) {
        match self.geo.schema.vars.iter().find(|v| v.name == name) {
            Some(v) => (v.dims.clone(), v.dtype),
            None => (vec![self.geo.schema.time_dim.clone()], StreamDType::F64),
        }
    }

    fn new_shard_buf(&self, ts: u64) -> ShardBuf {
        let st = self.geo.st as usize;
        let t0 = ts as usize * st;
        let mut bufs = BTreeMap::new();
        for v in self.my_vars() {
            let (o, s) = self.geo.var_box(&v.dims, t0, st);
            bufs.insert(v.name.clone(), NdBuf::zeros(o, s, v.dtype.size()));
        }
        if self.writes_time_coord() {
            bufs.insert(
                self.geo.schema.time_dim.clone(),
                NdBuf::zeros(vec![t0], vec![st], 8),
            );
        }
        ShardBuf {
            bufs,
            written: vec![false; st],
        }
    }

    fn ensure_open(&mut self, ts: u64) -> Result<()> {
        if self.open.contains_key(&ts) {
            return Ok(());
        }
        let mut sb = self.new_shard_buf(ts);
        if let Some(n) = self.geo.n_indexed() {
            if self.maybe_on_disk || self.flushed.contains(&ts) {
                for (name, buf) in &mut sb.bufs {
                    let (dims, dtype) = self.array_spec(name);
                    self.geo
                        .reload_box(&self.store, name, &dims, dtype, buf, n)?;
                }
            }
        }
        self.open.insert(ts, sb);
        Ok(())
    }

    fn time_extent(&self, ts: u64) -> u64 {
        self.geo.n_indexed().unwrap_or((ts + 1) * self.geo.st)
    }

    fn write_shard(&mut self, ts: u64) -> Result<()> {
        let ext = self.time_extent(ts);
        let Some(sb) = self.open.get(&ts) else {
            return Ok(());
        };
        let mut keys = Vec::new();
        for (name, buf) in &sb.bufs {
            let (dims, dtype) = self.array_spec(name);
            if !self.geo.owns(&dims) {
                continue;
            }
            keys.extend(
                self.geo
                    .write_box(&self.store, name, &dims, dtype, buf, ext)?,
            );
        }
        self.pending.objects.extend(keys);
        self.flushed.insert(ts);
        Ok(())
    }

    fn place_record(&mut self, index: u64, t: f64, arrays: &RecordArrays) -> Result<()> {
        let td = self.geo.schema.time_dim.clone();
        for name in arrays.keys() {
            match self.geo.schema.vars.iter().find(|v| v.name == *name) {
                None => {
                    return Err(err(format!(
                        "record has an array for unknown variable '{name}'"
                    )))
                }
                Some(v) if self.is_contributor() && !self.geo.partitioned(&v.dims) => {
                    return Err(err(format!(
                        "variable '{name}' is replicated (no region dim): only the owner writes it"
                    )))
                }
                _ => {}
            }
        }
        let mut srcs = Vec::new();
        for v in self.my_vars() {
            let a = arrays
                .get(&v.name)
                .ok_or_else(|| err(format!("record is missing the array for '{}'", v.name)))?;
            if a.dtype() != v.dtype {
                return Err(err(format!(
                    "'{}': got {} values for a {} variable",
                    v.name,
                    a.dtype().name(),
                    v.dtype.name()
                )));
            }
            let want = self.geo.local_len(&v.dims);
            if a.len() != want {
                return Err(err(format!(
                    "'{}': {} values != region size {want}",
                    v.name,
                    a.len()
                )));
            }
            let (o, s) = self.geo.var_box(&v.dims, index as usize, 1);
            srcs.push((
                v.name.clone(),
                NdBuf {
                    origin: o,
                    shape: s,
                    esize: v.dtype.size(),
                    data: a.to_ne_bytes(),
                },
            ));
        }
        if self.writes_time_coord() {
            srcs.push((
                td,
                NdBuf {
                    origin: vec![index as usize],
                    shape: vec![1],
                    esize: 8,
                    data: t.to_ne_bytes().to_vec(),
                },
            ));
        }
        let st = self.geo.st;
        let ts = index / st;
        self.ensure_open(ts)?;
        let sb = self.open.get_mut(&ts).expect("just opened");
        for (name, src) in &srcs {
            copy_overlap(
                src,
                sb.bufs.get_mut(name).expect("buffer per written array"),
            );
        }
        sb.written[(index % st) as usize] = true;
        let slots = match self.geo.n_indexed() {
            Some(n) => (n - ts * st).min(st),
            None => st,
        } as usize;
        let complete = sb.written[..slots].iter().all(|&w| w);
        self.pending.records.insert(index, t);
        if complete {
            self.write_shard(ts)?;
            self.open.remove(&ts);
            self.dirty.remove(&ts);
        } else {
            self.dirty.insert(ts);
        }
        Ok(())
    }

    /// Append one record (`Append` mode). `arrays` holds every variable this
    /// writer writes, region-shaped, without the time dim. Returns its index.
    ///
    /// # Errors
    /// In `Indexed` mode; for a `t` not after the previous one; for a missing,
    /// extra (a contributor supplying a replicated variable), mistyped or
    /// mis-sized array; on a store failure.
    pub fn write_record(&mut self, t: f64, arrays: &RecordArrays) -> Result<u64> {
        if self.geo.n_indexed().is_some() {
            return Err(err("write_record is append-mode only; use write_record_at"));
        }
        if let Some(prev) = self.last_written_t {
            if t.partial_cmp(&prev) != Some(std::cmp::Ordering::Greater) {
                return Err(err(format!(
                    "append t = {t} is not after the previous t = {prev}"
                )));
            }
        }
        let index = self.next_index;
        self.place_record(index, t, arrays)?;
        self.next_index += 1;
        self.last_written_t = Some(t);
        Ok(index)
    }

    /// Write record `index` (`Indexed` mode), in any order. `t` must equal
    /// `times[index]` bitwise. A committed record must be invalidated first.
    ///
    /// # Errors
    /// In `Append` mode, for an out-of-range index, a `t` mismatch, a
    /// committed record, or as [`write_record`](Self::write_record).
    pub fn write_record_at(&mut self, index: u64, t: f64, arrays: &RecordArrays) -> Result<()> {
        let Some(n) = self.geo.n_indexed() else {
            return Err(err(
                "write_record_at is indexed-mode only; use write_record",
            ));
        };
        if index >= n {
            return Err(err(format!(
                "record index {index} out of range (n_records = {n})"
            )));
        }
        let want = self.state.entries[index as usize].t;
        if want.to_bits() != t.to_bits() {
            return Err(err(format!(
                "record {index}: t = {t} != times[{index}] = {want}"
            )));
        }
        if self.record_committed(index)? {
            return Err(err(format!(
                "record {index} is committed; the owner must invalidate it before it is rewritten"
            )));
        }
        self.place_record(index, t, arrays)
    }

    fn refresh_view(&mut self) -> Result<()> {
        let m = read_output_manifest(self.store.as_ref())?;
        let (_, _, state) = from_manifest(&m)?;
        self.state = state;
        self.tokened_records.clear();
        self.tokened_statics.clear();
        Ok(())
    }

    fn record_committed(&mut self, i: u64) -> Result<bool> {
        // The owner's view is authoritative; a contributor also counts what it
        // has tokened (the owner may have committed it since the last read).
        if !self.is_contributor() {
            return Ok(self.state.entries[i as usize].committed);
        }
        let seen = self.state.entries[i as usize].committed || self.tokened_records.contains(&i);
        if seen {
            self.refresh_view()?;
            return Ok(self.state.entries[i as usize].committed);
        }
        Ok(seen)
    }

    fn static_committed(&mut self, name: &str) -> Result<bool> {
        if !self.is_contributor() {
            return Ok(self.state.statics.get(name).is_some_and(|s| s.0));
        }
        let seen = self.state.statics.get(name).is_some_and(|s| s.0)
            || self.tokened_statics.contains(name);
        if seen {
            self.refresh_view()?;
            return Ok(self.state.statics.get(name).is_some_and(|s| s.0));
        }
        Ok(seen)
    }

    /// Write a static (time-less) variable: region-shaped when partitioned,
    /// full-shaped (owner only) when replicated. Encoded and written now.
    ///
    /// # Errors
    /// For an unknown static, a contributor writing a replicated one, a
    /// dtype/size mismatch, a committed static, or a store failure.
    pub fn write_static(&mut self, name: &str, array: TypedArray<'_>) -> Result<()> {
        let v = self
            .geo
            .schema
            .statics
            .iter()
            .find(|v| v.name == name)
            .cloned()
            .ok_or_else(|| err(format!("unknown static '{name}'")))?;
        if self.is_contributor() && !self.geo.partitioned(&v.dims) {
            return Err(err(format!(
                "static '{name}' is replicated (no region dim): only the owner writes it"
            )));
        }
        if array.dtype() != v.dtype {
            return Err(err(format!(
                "static '{name}': got {} values for a {} static",
                array.dtype().name(),
                v.dtype.name()
            )));
        }
        let want = self.geo.local_len(&v.dims);
        if array.len() != want {
            return Err(err(format!(
                "static '{name}': {} values != region size {want}",
                array.len()
            )));
        }
        if self.static_committed(name)? {
            return Err(err(format!(
                "static '{name}' is committed; the owner must invalidate it before it is rewritten"
            )));
        }
        let (o, s) = self.geo.var_box(&v.dims, 0, 0);
        let buf = NdBuf {
            origin: o,
            shape: s,
            esize: v.dtype.size(),
            data: array.to_ne_bytes(),
        };
        if self.geo.owns(&v.dims) {
            let keys = self
                .geo
                .write_box(&self.store, name, &v.dims, v.dtype, &buf, 0)?;
            self.pending.objects.extend(keys);
        }
        self.pending.statics.insert(name.to_string());
        Ok(())
    }

    /// Store a small rank-local payload (`spec/writer.md` §7).
    ///
    /// # Errors
    /// On a v1 store (blobs need v2), for a bad key, or a store failure.
    pub fn write_blob(&mut self, scope: BlobScope, key: &str, bytes: &[u8]) -> Result<()> {
        if self.version == Version::V1 {
            return Err(err(
                "blobs need a v2 store: open a single writer as Owner { n_writers: 1 }",
            ));
        }
        if key.is_empty()
            || !key
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || "._-".contains(c))
        {
            return Err(err(format!("blob key '{key}' must match [A-Za-z0-9._-]+")));
        }
        if let (BlobScope::Record(i), Some(n)) = (scope, self.geo.n_indexed()) {
            if i >= n {
                return Err(err(format!(
                    "blob record {i} out of range (n_records = {n})"
                )));
            }
        }
        let path = scope.key(self.role.id(), key);
        let k = StoreKey::new(&path).map_err(|e| err(format!("invalid blob key '{path}': {e}")))?;
        self.store
            .set(&k, Bytes::from(bytes.to_vec()))
            .map_err(|e| err(format!("write {path}: {e}")))?;
        self.pending.blobs.insert((scope, key.to_string()));
        self.pending.objects.insert(path);
        Ok(())
    }

    fn take_token(&mut self) -> CommitToken {
        let p = std::mem::take(&mut self.pending);
        self.tokened_records.extend(p.records.keys().copied());
        self.tokened_statics.extend(p.statics.iter().cloned());
        let mut blobs: Vec<TokenBlob> = p
            .blobs
            .into_iter()
            .map(|(s, key)| {
                let (scope, record) = s.wire();
                TokenBlob {
                    scope: scope.to_string(),
                    record,
                    key,
                }
            })
            .collect();
        blobs.sort();
        CommitToken {
            schema: COMMIT_TOKEN_SCHEMA.to_string(),
            writer: self.role.id(),
            region: wire_region(&self.geo.region),
            fingerprint: self.fingerprint.clone(),
            records: p
                .records
                .into_iter()
                .map(|(index, t)| TokenRecord { index, t })
                .collect(),
            statics: p.statics.into_iter().collect(),
            blobs,
            objects: p.objects.into_iter().collect(),
        }
    }

    /// Write every dirty (partial) time shard of this writer's region and
    /// return the token describing what was written since the last token. A
    /// [`Role::Single`] writer also commits it (when it carries anything).
    ///
    /// # Errors
    /// A store failure, or (single writer) a [`commit`](Self::commit) error.
    pub fn flush(&mut self) -> Result<CommitToken> {
        for ts in std::mem::take(&mut self.dirty) {
            self.write_shard(ts)?;
        }
        let token = self.take_token();
        if self.role == Role::Single && !token.is_empty() {
            self.commit(std::slice::from_ref(&token))?;
        }
        Ok(token)
    }

    fn n_writers(&self) -> usize {
        match self.role {
            Role::Owner { n_writers } => n_writers,
            _ => 1,
        }
    }

    fn verify(&self, tokens: &[CommitToken]) -> std::result::Result<(), String> {
        let n = self.n_writers();
        let mut by_id: BTreeMap<usize, &CommitToken> = BTreeMap::new();
        for t in tokens {
            if t.writer >= n {
                return Err(format!(
                    "token from writer {} but n_writers = {n}",
                    t.writer
                ));
            }
            if by_id.insert(t.writer, t).is_some() {
                return Err(format!("two tokens from writer {}", t.writer));
            }
        }
        if let Some(missing) = (0..n).find(|i| !by_id.contains_key(i)) {
            return Err(format!("no token from writer {missing}"));
        }
        let toks: Vec<&CommitToken> = by_id.values().copied().collect();
        for t in &toks {
            if t.fingerprint != self.fingerprint {
                return Err(format!(
                    "writer {} has schema fingerprint {} != the owner's {}",
                    t.writer, t.fingerprint, self.fingerprint
                ));
            }
        }
        // regions: same dims, aligned, disjoint, tiling
        let rdims: Vec<&String> = toks[0].region.keys().collect();
        for t in &toks {
            if t.region.keys().collect::<Vec<_>>() != rdims {
                return Err(format!(
                    "writer {} names region dims {:?}, writer {} names {:?}",
                    t.writer,
                    t.region.keys().collect::<Vec<_>>(),
                    toks[0].writer,
                    rdims
                ));
            }
            let m: BTreeMap<String, Range<usize>> = t
                .region
                .iter()
                .map(|(d, r)| (d.clone(), r[0]..r[1]))
                .collect();
            validate_region(&self.geo.schema, &m)
                .map_err(|e| format!("writer {}: {e}", t.writer))?;
        }
        for (i, a) in toks.iter().enumerate() {
            for b in &toks[i + 1..] {
                let inter = rdims.iter().all(|d| {
                    let (x, y) = (a.region[*d], b.region[*d]);
                    x[0].max(y[0]) < x[1].min(y[1])
                });
                if inter {
                    return Err(format!(
                        "regions overlap: writer {} {:?} and writer {} {:?}",
                        a.writer, a.region, b.writer, b.region
                    ));
                }
            }
        }
        let vol = |t: &CommitToken| -> usize {
            rdims
                .iter()
                .map(|d| t.region[*d][1] - t.region[*d][0])
                .product()
        };
        let covered: usize = toks.iter().map(|t| vol(t)).sum();
        let total: usize = rdims.iter().map(|d| self.geo.dim_len[*d]).product();
        if covered != total {
            return Err(format!(
                "regions leave a gap: they cover {covered} of {total} cells over {rdims:?} ({})",
                toks.iter()
                    .map(|t| format!("writer {} {:?}", t.writer, t.region))
                    .collect::<Vec<_>>()
                    .join(", ")
            ));
        }
        // records: identical across writers
        let recs = &toks[0].records;
        for t in &toks[1..] {
            let same = t.records.len() == recs.len()
                && t.records
                    .iter()
                    .zip(recs)
                    .all(|(a, b)| a.index == b.index && a.t.to_bits() == b.t.to_bits());
            if !same {
                let f = |r: &[TokenRecord]| r.iter().map(|x| (x.index, x.t)).collect::<Vec<_>>();
                return Err(format!(
                    "writer {} carries records {:?}, writer {} carries {:?}",
                    t.writer,
                    f(&t.records),
                    toks[0].writer,
                    f(recs)
                ));
            }
        }
        match self.geo.n_indexed() {
            None => {
                for (k, r) in recs.iter().enumerate() {
                    let want = self.state.n_committed + k as u64;
                    if r.index != want {
                        return Err(format!("append records must continue the committed prefix: got index {}, expected {want}", r.index));
                    }
                }
            }
            Some(n) => {
                for r in recs {
                    if r.index >= n {
                        return Err(format!(
                            "record index {} out of range (n_records = {n})",
                            r.index
                        ));
                    }
                    let e = &self.state.entries[r.index as usize];
                    if e.t.to_bits() != r.t.to_bits() {
                        return Err(format!(
                            "record {}: t = {} != times[{}] = {}",
                            r.index, r.t, r.index, e.t
                        ));
                    }
                    if e.committed {
                        return Err(format!(
                            "record {} is already committed (invalidate it first)",
                            r.index
                        ));
                    }
                }
            }
        }
        // statics
        let all: BTreeSet<&String> = toks.iter().flat_map(|t| t.statics.iter()).collect();
        for name in all {
            let v = self
                .geo
                .schema
                .statics
                .iter()
                .find(|v| v.name == **name)
                .ok_or_else(|| format!("unknown static '{name}'"))?;
            if self.state.statics.get(name.as_str()).is_some_and(|s| s.0) {
                return Err(format!(
                    "static '{name}' is already committed (invalidate it first)"
                ));
            }
            let partitioned = v.dims.iter().any(|d| rdims.contains(&d));
            for t in &toks {
                let has = t.statics.contains(name);
                if partitioned && !has {
                    return Err(format!(
                        "static '{name}' is partitioned but writer {} did not write it",
                        t.writer
                    ));
                }
                if !partitioned && has && t.writer != 0 {
                    return Err(format!(
                        "static '{name}' is replicated but writer {} wrote it",
                        t.writer
                    ));
                }
            }
        }
        for t in &toks {
            for b in &t.blobs {
                BlobScope::from_wire(&b.scope, b.record)
                    .map_err(|e| format!("writer {}: {e}", t.writer))?;
            }
        }
        Ok(())
    }

    /// Owner (or single writer): verify the writers' tokens (one per writer id,
    /// the owner's own included) and commit them — shapes, then the manifest,
    /// LAST (`spec/writer.md` §5.3).
    ///
    /// # Errors
    /// [`Error::Commit`] naming the gap, the overlap or the divergent writer
    /// (nothing is written); [`Error::Format`] for a contributor or a store
    /// failure.
    pub fn commit(&mut self, tokens: &[CommitToken]) -> Result<CommitReceipt> {
        if self.is_contributor() {
            return Err(err("only the owner commits"));
        }
        self.verify(tokens)
            .map_err(|detail| Error::Commit { detail })?;
        let mut toks: Vec<&CommitToken> = tokens.iter().collect();
        toks.sort_by_key(|t| t.writer);
        let recs = toks[0].records.clone();
        let st = self.geo.st;
        match self.geo.n_indexed() {
            None => {
                for r in &recs {
                    self.state.push_append(r.index, r.t, st);
                }
                if !recs.is_empty() {
                    self.state.n_committed += recs.len() as u64;
                    self.write_shapes(self.state.n_committed)?;
                }
            }
            Some(_) => {
                for r in &recs {
                    let e = &mut self.state.entries[r.index as usize];
                    e.committed = true;
                    e.generation += 1;
                }
                self.state.rebuild_indexed_shards(st);
            }
        }
        let mut statics: BTreeSet<String> = BTreeSet::new();
        for t in &toks {
            statics.extend(t.statics.iter().cloned());
            for b in &t.blobs {
                let scope = BlobScope::from_wire(&b.scope, b.record)?;
                self.state.blobs.insert(BlobRec {
                    scope,
                    writer: t.writer,
                    key: b.key.clone(),
                });
            }
        }
        for s in &statics {
            let e = self.state.statics.entry(s.clone()).or_insert((false, 0));
            e.0 = true;
            e.1 += 1;
        }
        self.state.writers = toks.iter().map(|t| (t.writer, t.region.clone())).collect();
        self.state.commit_seq += 1;
        self.write_manifest()?;
        Ok(CommitReceipt {
            commit_seq: self.state.commit_seq,
            records: recs.iter().map(|r| r.index).collect(),
            statics: statics.into_iter().collect(),
            n_committed: self.state.n_committed,
        })
    }

    /// Owner: mark committed records uncommitted (`Indexed` mode) so they may be
    /// rewritten, and write the manifest. Their `record` blobs are uncommitted too.
    ///
    /// # Errors
    /// For a contributor, in `Append` mode, an out-of-range index, or a store failure.
    pub fn invalidate(&mut self, indices: &[u64]) -> Result<()> {
        if self.is_contributor() {
            return Err(err("only the owner invalidates"));
        }
        let Some(n) = self.geo.n_indexed() else {
            return Err(err(
                "invalidate is indexed-mode only (append mode truncates with resume)",
            ));
        };
        for &i in indices {
            if i >= n {
                return Err(err(format!(
                    "record index {i} out of range (n_records = {n})"
                )));
            }
        }
        for &i in indices {
            self.state.entries[i as usize].committed = false;
        }
        let gone: BTreeSet<u64> = indices.iter().copied().collect();
        self.state
            .blobs
            .retain(|b| !matches!(b.scope, BlobScope::Record(i) if gone.contains(&i)));
        self.state.rebuild_indexed_shards(self.geo.st);
        self.state.commit_seq += 1;
        self.write_manifest()
    }

    /// Owner: mark committed statics uncommitted so they may be rewritten.
    ///
    /// # Errors
    /// For a contributor, an unknown static, or a store failure.
    pub fn invalidate_statics(&mut self, names: &[&str]) -> Result<()> {
        if self.is_contributor() {
            return Err(err("only the owner invalidates"));
        }
        for n in names {
            if !self.state.statics.contains_key(*n) {
                return Err(err(format!("unknown static '{n}'")));
            }
        }
        for n in names {
            if let Some(s) = self.state.statics.get_mut(*n) {
                s.0 = false;
            }
        }
        self.state.commit_seq += 1;
        self.write_manifest()
    }

    /// Finish the handle. Single: flush (committing) and rewrite the manifest.
    /// Owner: flush its own data and commit it with `tokens` — the OTHER
    /// writers' final tokens. Contributor: `tokens` must be empty and nothing
    /// may be unflushed (deliver the last token first). Returns the manifest
    /// for the owner / single writer.
    ///
    /// # Errors
    /// As [`flush`](Self::flush) / [`commit`](Self::commit); a contributor with
    /// unflushed writes.
    pub fn close(mut self, tokens: &[CommitToken]) -> Result<Option<Value>> {
        match self.role {
            Role::Contributor { .. } => {
                if !tokens.is_empty() {
                    return Err(err("a contributor's close takes no tokens"));
                }
                if self.pending.has_content() || !self.dirty.is_empty() {
                    return Err(err("contributor closed with unflushed writes: flush and deliver the token first"));
                }
                Ok(None)
            }
            Role::Single => {
                if !tokens.is_empty() {
                    return Err(err("a single writer's close takes no tokens"));
                }
                self.flush()?;
                self.write_manifest()?;
                Ok(Some(self.manifest_json()))
            }
            Role::Owner { .. } => {
                let own = self.flush()?;
                let mut all = vec![own];
                all.extend(tokens.iter().cloned());
                self.commit(&all)?;
                Ok(Some(self.manifest_json()))
            }
        }
    }

    /// The writer's region, as bound at open.
    pub fn region(&self) -> Region {
        Region(
            self.geo
                .region
                .iter()
                .map(|(d, r)| (d.clone(), r.clone()))
                .collect(),
        )
    }

    /// The schema fingerprint.
    pub fn fingerprint(&self) -> &str {
        &self.fingerprint
    }

    // --- the manifest -------------------------------------------------------

    fn manifest_json(&self) -> Value {
        let s = &self.geo.schema;
        let indexed = self.geo.n_indexed();
        let n = indexed.unwrap_or(self.state.n_committed);
        let dims: Vec<Value> = s
            .dims
            .iter()
            .map(|(k, l)| json!({"name": k, "length": if *k == s.time_dim { n as usize } else { *l }}))
            .collect();
        let entry = |v: &StreamVar| {
            let mut e = json!({"name": v.name, "dims": v.dims, "dtype": v.dtype.name()});
            // Omitted when empty, so a store that states nothing serializes
            // byte-for-byte what v1 always did (readers pin the v1 label).
            if !v.attrs.is_empty() {
                e["attrs"] = Value::Object(v.attrs.clone());
            }
            e
        };
        let vars: Vec<Value> = s.vars.iter().map(entry).collect();
        let time_shards: Vec<Value> = self
            .state
            .time_shards
            .iter()
            .map(|t| json!({"index": t.index, "t_start": t.t_start, "t_end": t.t_end, "n": t.n}))
            .collect();
        let mut m = json!({
            "schema": if self.version == Version::V1 { OUTPUT_MANIFEST_V1 } else { OUTPUT_MANIFEST_V2 },
            "base_url": self.base_url,
            "format": "zarr",
            "zarr_format": 3,
            "profile": s.profile,
            "codec": codec_meta(&self.geo.codec),
            "time_dim": s.time_dim,
            "dims": dims,
            "vars": vars,
            "chunk_shape": s.chunk_shape,
            "shard_shape": s.shard_shape,
            "time_shards": time_shards,
            "last_t": self.state.last_t,
            "n_records": n,
            "created_at": crate::clock::now_rfc3339(),
        });
        if self.version == Version::V1 {
            return m;
        }
        m["schema_fingerprint"] = json!(self.fingerprint);
        m["time_mode"] = match indexed {
            None => json!({"kind": "append"}),
            Some(n) => json!({"kind": "indexed", "n_records": n}),
        };
        m["writers"] = Value::Array(
            self.state
                .writers
                .iter()
                .map(|(id, r)| json!({"id": id, "region": r}))
                .collect(),
        );
        let mut records = json!({"committed": committed_ranges(&self.state, indexed.is_some())});
        if indexed.is_some() {
            records["entries"] = Value::Array(
                self.state
                    .entries
                    .iter()
                    .enumerate()
                    .map(|(i, e)| json!({"index": i, "t": e.t, "committed": e.committed, "generation": e.generation}))
                    .collect(),
            );
        }
        m["records"] = records;
        m["statics"] = Value::Array(
            s.statics
                .iter()
                .map(|v| {
                    let (c, g) = self
                        .state
                        .statics
                        .get(&v.name)
                        .copied()
                        .unwrap_or((false, 0));
                    let mut e = entry(v);
                    e["committed"] = json!(c);
                    e["generation"] = json!(g);
                    e
                })
                .collect(),
        );
        let mut blobs: Vec<&BlobRec> = self.state.blobs.iter().collect();
        blobs.sort_by_key(|b| {
            let (scope, record) = b.scope.wire();
            (scope, record, b.writer, b.key.clone())
        });
        m["blobs"] = Value::Array(
            blobs
                .into_iter()
                .map(|b| {
                    let (scope, record) = b.scope.wire();
                    json!({"scope": scope, "record": record, "writer": b.writer, "key": b.key})
                })
                .collect(),
        );
        m["commit_seq"] = json!(self.state.commit_seq);
        m
    }

    fn write_manifest(&self) -> Result<()> {
        if self.is_contributor() {
            return Err(err("a contributor never writes the manifest"));
        }
        set_json(self.store.as_ref(), MANIFEST_KEY, &self.manifest_json())
    }
}

// --- read-back -----------------------------------------------------------------

/// Read and parse `output_manifest.json` (v1 or v2).
///
/// # Errors
/// [`Error::Format`] when the store has no manifest or it does not parse.
pub fn read_output_manifest<S: ReadableStorageTraits + ?Sized>(store: &S) -> Result<Value> {
    get_json(store, MANIFEST_KEY)?.ok_or_else(|| err("the store has no output_manifest.json"))
}

fn record_is_committed(m: &Value, index: u64) -> bool {
    if m["schema"] == OUTPUT_MANIFEST_V1 {
        // Julia/Rust spell the v1 count `n_records`, Python `total_records`.
        let n = m["n_records"]
            .as_u64()
            .or_else(|| m["total_records"].as_u64());
        return n.is_some_and(|n| index < n);
    }
    m["records"]["committed"].as_array().is_some_and(|rs| {
        rs.iter().any(|r| {
            r[0].as_u64().is_some_and(|a| a <= index) && r[1].as_u64().is_some_and(|b| index < b)
        })
    })
}

fn read_box<S>(
    store: Arc<S>,
    name: &str,
    time: Option<(&str, u64)>,
    region: &Region,
) -> Result<TypedVec>
where
    S: ReadableStorageTraits + 'static,
{
    let meta = get_json(store.as_ref(), &format!("{name}/zarr.json"))?
        .ok_or_else(|| err(format!("no array '{name}' in the store")))?;
    let dims: Vec<String> = serde_json::from_value(meta["dimension_names"].clone())
        .map_err(|_| err(format!("array '{name}' has no dimension_names")))?;
    let shape: Vec<u64> = serde_json::from_value(meta["shape"].clone())
        .map_err(|_| err(format!("array '{name}' has no shape")))?;
    let dtype = StreamDType::parse(meta["data_type"].as_str().unwrap_or(""))?;
    let reg = region.to_map()?;
    let ranges: Vec<Range<u64>> = dims
        .iter()
        .zip(&shape)
        .map(|(d, &len)| match (time, reg.get(d)) {
            (Some((td, i)), _) if td == d => Ok(i..i + 1),
            (_, Some(r)) if r.start < r.end && r.end as u64 <= len => {
                Ok(r.start as u64..r.end as u64)
            }
            (_, Some(r)) => Err(err(format!(
                "region {d} [{}, {}) is outside [0, {len})",
                r.start, r.end
            ))),
            (_, None) => Ok(0..len),
        })
        .collect::<Result<_>>()?;
    let array = Array::open(store, &format!("/{name}"))
        .map_err(|e| err(format!("open array '{name}': {e}")))?;
    retrieve(&array, &ArraySubset::new_with_ranges(&ranges), dtype)
}

/// Read the `region` of committed record `index` of variable `var` (shape: the
/// variable's dims minus time, region-sized). Committed data only; read
/// directly from the store, never through the content-addressed cache.
///
/// # Errors
/// [`Error::Format`] for an uncommitted record, an unknown variable, a region
/// outside the array, or a store failure.
pub fn read_record<S>(store: Arc<S>, var: &str, index: u64, region: &Region) -> Result<TypedVec>
where
    S: ReadableStorageTraits + 'static,
{
    let m = read_output_manifest(store.as_ref())?;
    if !record_is_committed(&m, index) {
        return Err(err(format!("record {index} is not committed")));
    }
    let td = m["time_dim"].as_str().unwrap_or_default().to_string();
    read_box(store, var, Some((&td, index)), region)
}

/// Read the `region` of committed static `name`.
///
/// # Errors
/// [`Error::Format`] for an uncommitted or unknown static, or a store failure.
pub fn read_static<S>(store: Arc<S>, name: &str, region: &Region) -> Result<TypedVec>
where
    S: ReadableStorageTraits + 'static,
{
    let m = read_output_manifest(store.as_ref())?;
    let committed = m["statics"].as_array().is_some_and(|a| {
        a.iter()
            .any(|s| s["name"] == name && s["committed"] == true)
    });
    if !committed {
        return Err(err(format!("static '{name}' is not committed")));
    }
    read_box(store, name, None, region)
}

/// Read a committed blob.
///
/// # Errors
/// [`Error::Format`] for an uncommitted blob or a store failure.
pub fn read_blob<S: ReadableStorageTraits + ?Sized>(
    store: &S,
    scope: BlobScope,
    writer: usize,
    key: &str,
) -> Result<Vec<u8>> {
    let m = read_output_manifest(store)?;
    let (sc, rec) = scope.wire();
    let committed = m["blobs"].as_array().is_some_and(|a| {
        a.iter().any(|b| {
            b["scope"] == sc
                && b["record"].as_u64() == rec
                && b["writer"].as_u64() == Some(writer as u64)
                && b["key"] == key
        })
    });
    if !committed {
        return Err(err(format!(
            "blob {sc}/{rec:?}/{writer}/{key} is not committed"
        )));
    }
    let path = scope.key(writer, key);
    let k = StoreKey::new(&path).map_err(|e| err(format!("invalid blob key '{path}': {e}")))?;
    store
        .get(&k)
        .map_err(|e| err(format!("read {path}: {e}")))?
        .map(|b| b.to_vec())
        .ok_or_else(|| err(format!("committed blob {path} is missing from the store")))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn canonical_json_matches_python() {
        let v = json!({"b": [1, "x\u{e9}\n"], "a": {"d": 2, "c": null}});
        let mut s = String::new();
        canonical_json(&v, &mut s);
        // python3 -c 'import json;print(json.dumps({"b":[1,"xé\n"],"a":{"d":2,"c":None}},sort_keys=True,separators=(",",":"),ensure_ascii=True))'
        assert_eq!(s, "{\"a\":{\"c\":null,\"d\":2},\"b\":[1,\"x\\u00e9\\n\"]}");
    }

    #[test]
    fn copy_overlap_places_a_sub_box() {
        let src = NdBuf {
            origin: vec![1, 2],
            shape: vec![2, 2],
            esize: 1,
            data: vec![1, 2, 3, 4],
        };
        let mut dst = NdBuf::zeros(vec![0, 0], vec![4, 4], 1);
        copy_overlap(&src, &mut dst);
        assert_eq!(
            dst.data,
            vec![0, 0, 0, 0, 0, 0, 1, 2, 0, 0, 3, 4, 0, 0, 0, 0]
        );
    }
}
