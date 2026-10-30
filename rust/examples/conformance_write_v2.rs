//! Rust track's driver for the **v2 write-conformance** corpus
//! (`conformance/write_spec_v2.json`, `spec/writer.md` §10).
//!
//! `write` executes every case's script of writer operations (create / join /
//! write / flush / commit / resume / close …) against a fresh local store with
//! [`ZarrStreamWriter`], recording which ops failed and how; `read` reads every
//! case's store back through the read-back API ([`read_record`],
//! [`read_static`], [`read_blob`]) into an `earthsciio/write-v2-dump/v1` dump.
//! `conformance/crosscheck_write_v2.py` checks both against the oracle, bitwise.
//!
//! Usage:
//!   cargo run --example conformance_write_v2 -- write SPEC OUT_ROOT
//!   cargo run --example conformance_write_v2 -- read  SPEC STORES_ROOT WRITER_LABEL OUT_DUMP

use std::collections::{BTreeMap, HashMap};
use std::path::{Path, PathBuf};
use std::sync::Arc;

use earthsciio::{
    read_blob, read_output_manifest, read_record, read_static, BlobScope, CommitToken, Error,
    Region, Role, StreamDType, StreamSchema, StreamVar, TimeMode, TypedArray, TypedVec, WriteCoord,
    ZarrStreamWriter,
};
use serde_json::{json, Map, Value};
use zarrs::filesystem::FilesystemStore;

type Writer = ZarrStreamWriter<FilesystemStore>;

fn read_json(path: &Path) -> Value {
    let bytes = std::fs::read(path).unwrap_or_else(|e| panic!("read {}: {e}", path.display()));
    serde_json::from_slice(&bytes).unwrap_or_else(|e| panic!("parse {}: {e}", path.display()))
}

fn umap(v: &Value) -> BTreeMap<String, usize> {
    v.as_object()
        .unwrap()
        .iter()
        .map(|(k, x)| (k.clone(), x.as_u64().unwrap() as usize))
        .collect()
}

fn strs(v: &Value) -> Vec<String> {
    v.as_array()
        .unwrap()
        .iter()
        .map(|s| s.as_str().unwrap().to_string())
        .collect()
}

fn parse_vars(v: &Value) -> Vec<StreamVar> {
    v.as_array()
        .map(|a| a.as_slice())
        .unwrap_or(&[])
        .iter()
        .map(|e| StreamVar {
            name: e["name"].as_str().unwrap().to_string(),
            dims: strs(&e["dims"]),
            dtype: StreamDType::parse(e["dtype"].as_str().unwrap()).unwrap(),
            attrs: e["attrs"].as_object().cloned().unwrap_or_default(),
        })
        .collect()
}

fn build_schema(s: &Value) -> StreamSchema {
    let time_mode = match s["time_mode"]["kind"].as_str().unwrap() {
        "append" => TimeMode::Append,
        _ => {
            let times: Vec<f64> = s["time_mode"]["times"]
                .as_array()
                .unwrap()
                .iter()
                .map(|t| t.as_f64().unwrap())
                .collect();
            TimeMode::Indexed {
                n_records: times.len(),
                times,
            }
        }
    };
    StreamSchema {
        dims: s["dims"]
            .as_array()
            .unwrap()
            .iter()
            .map(|d| {
                (
                    d[0].as_str().unwrap().to_string(),
                    d[1].as_u64().unwrap() as usize,
                )
            })
            .collect(),
        time_dim: s["time_dim"].as_str().unwrap().to_string(),
        chunk_shape: umap(&s["chunk_shape"]),
        shard_shape: umap(&s["shard_shape"]),
        coords: s["coords"]
            .as_array()
            .unwrap()
            .iter()
            .map(|c| WriteCoord {
                name: c["name"].as_str().unwrap().to_string(),
                values: c["values"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .map(|x| x.as_f64().unwrap())
                    .collect(),
                attrs: c["attrs"].as_object().cloned().unwrap_or_default(),
            })
            .collect(),
        vars: parse_vars(&s["vars"]),
        statics: parse_vars(&s["statics"]),
        time_mode,
        group_attrs: s["group_attrs"].as_object().cloned().unwrap_or_default(),
        profile: s["profile"].as_str().unwrap().to_string(),
    }
}

fn parse_role(v: &Value) -> Role {
    if v == "single" {
        return Role::Single;
    }
    if let Some(n) = v.get("owner") {
        return Role::Owner {
            n_writers: n.as_u64().unwrap() as usize,
        };
    }
    Role::Contributor {
        id: v["contributor"].as_u64().unwrap() as usize,
    }
}

fn parse_region(v: &Value) -> Region {
    Region::new(v.as_object().unwrap().iter().map(|(d, r)| {
        (
            d.clone(),
            r[0].as_u64().unwrap() as usize..r[1].as_u64().unwrap() as usize,
        )
    }))
}

/// The generator of `spec/writer.md` §10.
struct Gen {
    p: Vec<i64>,
    stride: i64,
    off: f64,
    modulus: i64,
}

impl Gen {
    fn new(g: &Value) -> Self {
        Self {
            p: g["P"]
                .as_array()
                .unwrap()
                .iter()
                .map(|x| x.as_i64().unwrap())
                .collect(),
            stride: g["record_stride"].as_i64().unwrap(),
            off: g["float_offset"].as_f64().unwrap(),
            modulus: g["uint8_modulus"].as_i64().unwrap(),
        }
    }

    /// Values over the box `ranges` (global, C order) for `dtype`.
    fn array(
        &self,
        dtype: StreamDType,
        seed: i64,
        r: i64,
        ranges: &[std::ops::Range<usize>],
    ) -> TypedVec {
        let lens: Vec<usize> = ranges.iter().map(|r| r.len()).collect();
        let total: usize = lens.iter().product();
        let bases: Vec<i64> = (0..total)
            .map(|lin| {
                // C-order unravel of `lin` over the box, then the global index.
                let mut rem = lin;
                let mut sum = 0i64;
                for d in (0..lens.len()).rev() {
                    let i = ranges[d].start + rem % lens[d];
                    rem /= lens[d];
                    sum += (i as i64 + 1) * self.p[d];
                }
                seed + self.stride * r + sum
            })
            .collect();
        match dtype {
            StreamDType::F64 => TypedVec::F64(bases.iter().map(|&b| b as f64 + self.off).collect()),
            StreamDType::F32 => TypedVec::F32(
                bases
                    .iter()
                    .map(|&b| (b as f64 + self.off) as f32)
                    .collect(),
            ),
            StreamDType::I32 => TypedVec::I32(bases.iter().map(|&b| b as i32).collect()),
            StreamDType::I64 => TypedVec::I64(bases),
            StreamDType::U8 => {
                TypedVec::U8(bases.iter().map(|&b| (b % self.modulus) as u8).collect())
            }
        }
    }
}

struct Ctx<'a> {
    schema_json: &'a Value,
    schema: StreamSchema,
    gen: &'a Gen,
    store: Arc<FilesystemStore>,
    base: String,
    writers: HashMap<String, (Writer, Role, Region)>,
    tokens: HashMap<String, CommitToken>,
}

impl Ctx<'_> {
    fn seed(&self, name: &str) -> i64 {
        let all = self.schema_json["vars"]
            .as_array()
            .unwrap()
            .iter()
            .chain(self.schema_json["statics"].as_array().unwrap());
        for v in all {
            if v["name"] == name {
                return v["seed"].as_i64().unwrap();
            }
        }
        panic!("no seed for {name}");
    }

    fn ranges(&self, dims: &[String], region: &Region) -> Vec<std::ops::Range<usize>> {
        dims.iter()
            .filter(|d| **d != self.schema.time_dim)
            .map(|d| {
                region
                    .0
                    .iter()
                    .find(|(rd, _)| rd == d)
                    .map(|(_, r)| r.clone())
                    .unwrap_or(0..self.schema.dims.iter().find(|(n, _)| n == d).unwrap().1)
            })
            .collect()
    }

    fn record_arrays(&self, role: Role, region: &Region, r: i64) -> Vec<(String, TypedVec)> {
        let rdims: Vec<&String> = region.0.iter().map(|(d, _)| d).collect();
        self.schema
            .vars
            .iter()
            .filter(|v| {
                !matches!(role, Role::Contributor { .. })
                    || v.dims.iter().any(|d| rdims.contains(&d))
            })
            .map(|v| {
                (
                    v.name.clone(),
                    self.gen.array(
                        v.dtype,
                        self.seed(&v.name),
                        r,
                        &self.ranges(&v.dims, region),
                    ),
                )
            })
            .collect()
    }

    fn run(&mut self, op: &Value) -> earthsciio::Result<()> {
        let w = op["writer"].as_str().unwrap_or_default().to_string();
        let tok_list = |c: &Self| -> Vec<CommitToken> {
            op["tokens"]
                .as_array()
                .unwrap()
                .iter()
                .map(|t| c.tokens[t.as_str().unwrap()].clone())
                .collect()
        };
        match op["op"].as_str().unwrap() {
            "create" | "join" | "resume" => {
                let role = parse_role(&op["role"]);
                let region = parse_region(&op["region"]);
                let h = match op["op"].as_str().unwrap() {
                    "create" => Writer::create(
                        self.store.clone(),
                        &self.base,
                        self.schema.clone(),
                        role,
                        region.clone(),
                    )?,
                    "join" => Writer::join(self.store.clone(), &self.base, role, region.clone())?,
                    _ => Writer::resume(
                        self.store.clone(),
                        &self.base,
                        role,
                        region.clone(),
                        op["at"].as_f64(),
                    )?,
                };
                self.writers.insert(w, (h, role, region));
            }
            "write_record" | "write_record_at" => {
                let r = op["r"].as_i64().unwrap();
                let (role, region) = {
                    let e = &self.writers[&w];
                    (e.1, e.2.clone())
                };
                let owned = self.record_arrays(role, &region, r);
                let arrays: BTreeMap<String, TypedArray> = owned
                    .iter()
                    .map(|(n, v)| (n.clone(), v.as_array()))
                    .collect();
                let h = &mut self.writers.get_mut(&w).unwrap().0;
                if op["op"] == "write_record" {
                    h.write_record(op["t"].as_f64().unwrap(), &arrays)?;
                } else {
                    let i = op["index"].as_u64().unwrap();
                    let TimeMode::Indexed { times, .. } = &self.schema.time_mode else {
                        unreachable!()
                    };
                    h.write_record_at(i, times[i as usize], &arrays)?;
                }
            }
            "write_static" => {
                let name = op["name"].as_str().unwrap();
                let v = self
                    .schema
                    .statics
                    .iter()
                    .find(|v| v.name == name)
                    .unwrap()
                    .clone();
                let region = self.writers[&w].2.clone();
                let vals =
                    self.gen
                        .array(v.dtype, self.seed(name), 0, &self.ranges(&v.dims, &region));
                self.writers
                    .get_mut(&w)
                    .unwrap()
                    .0
                    .write_static(name, vals.as_array())?;
            }
            "write_blob" => {
                let scope = if op["scope"] == "static" {
                    BlobScope::Static
                } else {
                    BlobScope::Record(op["record"].as_u64().unwrap())
                };
                let hex = op["hex"].as_str().unwrap();
                let bytes: Vec<u8> = (0..hex.len())
                    .step_by(2)
                    .map(|i| u8::from_str_radix(&hex[i..i + 2], 16).unwrap())
                    .collect();
                self.writers.get_mut(&w).unwrap().0.write_blob(
                    scope,
                    op["key"].as_str().unwrap(),
                    &bytes,
                )?;
            }
            "flush" => {
                let t = self.writers.get_mut(&w).unwrap().0.flush()?;
                // Round-trip through the wire form, as an MPI transport would.
                let t = CommitToken::from_json(&t.to_json())?;
                self.tokens
                    .insert(op["token"].as_str().unwrap().to_string(), t);
            }
            "commit" => {
                let toks = tok_list(self);
                self.writers.get_mut(&w).unwrap().0.commit(&toks)?;
            }
            "invalidate" => {
                let idx: Vec<u64> = op["indices"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .map(|i| i.as_u64().unwrap())
                    .collect();
                self.writers.get_mut(&w).unwrap().0.invalidate(&idx)?;
            }
            "close" => {
                let toks = tok_list(self);
                let (h, _, _) = self.writers.remove(&w).unwrap();
                h.close(&toks)?;
            }
            "drop" => {
                self.writers.remove(&w);
            }
            other => panic!("unknown op {other}"),
        }
        Ok(())
    }
}

fn kind(e: &Error) -> &'static str {
    match e {
        Error::RegionNotShardAligned { .. } => "RegionNotShardAligned",
        Error::Commit { .. } => "CommitError",
        _ => "Error",
    }
}

fn write_all(spec: &Value, out: &Path) {
    let gen = Gen::new(&spec["generator"]);
    for case in spec["cases"].as_array().unwrap() {
        let name = case["name"].as_str().unwrap();
        let dir = out.join(name);
        let store_dir = dir.join("store");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&store_dir).unwrap();
        let mut ctx = Ctx {
            schema_json: &case["schema"],
            schema: build_schema(&case["schema"]),
            gen: &gen,
            store: Arc::new(FilesystemStore::new(&store_dir).unwrap()),
            base: store_dir.to_string_lossy().into_owned(),
            writers: HashMap::new(),
            tokens: HashMap::new(),
        };
        let mut results = Vec::new();
        for (k, op) in case["ops"].as_array().unwrap().iter().enumerate() {
            let r = ctx.run(op);
            results.push(match &r {
                Ok(()) => json!({"index": k, "op": op["op"], "ok": true, "error": null, "message": ""}),
                Err(e) => json!({"index": k, "op": op["op"], "ok": false, "error": kind(e), "message": e.to_string()}),
            });
        }
        let res = json!({"case": name, "ops": results});
        std::fs::write(
            dir.join("result.json"),
            serde_json::to_vec_pretty(&res).unwrap(),
        )
        .unwrap();
        eprintln!("[rust-v2] wrote case {name}");
    }
}

fn dump_values(v: &TypedVec) -> Vec<Value> {
    match v {
        TypedVec::F64(x) => x.iter().map(|&a| json!(a)).collect(),
        TypedVec::F32(x) => x.iter().map(|&a| json!(f64::from(a))).collect(),
        TypedVec::I32(x) => x.iter().map(|&a| json!(a)).collect(),
        TypedVec::I64(x) => x.iter().map(|&a| json!(a)).collect(),
        TypedVec::U8(x) => x.iter().map(|&a| json!(a)).collect(),
    }
}

fn read_case(store_dir: &Path) -> earthsciio::Result<Value> {
    let store = Arc::new(FilesystemStore::new(store_dir).unwrap());
    let m = read_output_manifest(store.as_ref())?;
    let v2 = m["schema"] == "earthsciio/output-manifest/v2";
    let committed: Vec<u64> = if v2 {
        m["records"]["committed"]
            .as_array()
            .unwrap()
            .iter()
            .flat_map(|r| r[0].as_u64().unwrap()..r[1].as_u64().unwrap())
            .collect()
    } else {
        // v1: Julia/Rust spell the count `n_records`, Python `total_records`.
        let n = m["n_records"]
            .as_u64()
            .or_else(|| m["total_records"].as_u64());
        (0..n.unwrap()).collect()
    };
    let generations: Value = match m["records"]["entries"].as_array() {
        Some(es) => Value::Object(
            es.iter()
                .map(|e| (e["index"].to_string(), e["generation"].clone()))
                .collect(),
        ),
        None => Value::Null,
    };
    let td = m["time_dim"].as_str().unwrap();
    // dims are `{name, length}` (Julia/Rust, v2) or `[name, length]` (Python v1).
    let dimlen: BTreeMap<String, u64> = m["dims"]
        .as_array()
        .unwrap()
        .iter()
        .map(|d| match d.as_array() {
            Some(p) => (p[0].as_str().unwrap().to_string(), p[1].as_u64().unwrap()),
            None => (
                d["name"].as_str().unwrap().to_string(),
                d["length"].as_u64().unwrap(),
            ),
        })
        .collect();
    let entry = |dims: &[String], v: &TypedVec| -> Value {
        let shape: Vec<u64> = dims
            .iter()
            .filter(|d| *d != td)
            .map(|d| dimlen[d])
            .collect();
        json!({"dtype": v.dtype().name(), "shape": shape, "values": dump_values(v)})
    };
    let mut records = Map::new();
    for var in m["vars"].as_array().unwrap() {
        let name = var["name"].as_str().unwrap();
        let dims = strs(&var["dims"]);
        let mut per = Map::new();
        for &i in &committed {
            let v = read_record(store.clone(), name, i, &Region::whole())?;
            per.insert(i.to_string(), entry(&dims, &v));
        }
        records.insert(name.to_string(), Value::Object(per));
    }
    let mut statics = Map::new();
    for s in m["statics"].as_array().map(|a| a.as_slice()).unwrap_or(&[]) {
        if s["committed"] != true {
            continue;
        }
        let name = s["name"].as_str().unwrap();
        let v = read_static(store.clone(), name, &Region::whole())?;
        statics.insert(name.to_string(), entry(&strs(&s["dims"]), &v));
    }
    let mut blobs = Vec::new();
    for b in m["blobs"].as_array().map(|a| a.as_slice()).unwrap_or(&[]) {
        let scope = if b["scope"] == "static" {
            BlobScope::Static
        } else {
            BlobScope::Record(b["record"].as_u64().unwrap())
        };
        let writer = b["writer"].as_u64().unwrap() as usize;
        let key = b["key"].as_str().unwrap();
        let bytes = read_blob(store.as_ref(), scope, writer, key)?;
        let hex: String = bytes.iter().map(|x| format!("{x:02x}")).collect();
        blobs.push(json!({"scope": b["scope"], "record": b["record"], "writer": writer, "key": key, "hex": hex}));
    }
    Ok(json!({
        "manifest_schema": m["schema"],
        "commit_seq": if v2 { m["commit_seq"].clone() } else { Value::Null },
        "committed": committed,
        "generations": generations,
        "records": records,
        "statics": statics,
        "blobs": blobs,
    }))
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let usage =
        "usage: conformance_write_v2 write SPEC OUT_ROOT | read SPEC STORES_ROOT WRITER OUT_DUMP";
    let spec = read_json(Path::new(args.get(1).expect(usage)));
    match args.first().map(String::as_str) {
        Some("write") => write_all(&spec, &PathBuf::from(args.get(2).expect(usage))),
        Some("read") => {
            let root = PathBuf::from(args.get(2).expect(usage));
            let mut cases = Map::new();
            for case in spec["cases"].as_array().unwrap() {
                let name = case["name"].as_str().unwrap();
                let got = read_case(&root.join(name).join("store"))
                    .unwrap_or_else(|e| json!({"error": e.to_string()}));
                cases.insert(name.to_string(), got);
            }
            let dump = json!({
                "schema": "earthsciio/write-v2-dump/v1",
                "reader": "rust",
                "writer": args.get(3).expect(usage),
                "cases": cases,
            });
            std::fs::write(
                args.get(4).expect(usage),
                serde_json::to_vec_pretty(&dump).unwrap(),
            )
            .unwrap();
        }
        _ => panic!("{usage}"),
    }
}
