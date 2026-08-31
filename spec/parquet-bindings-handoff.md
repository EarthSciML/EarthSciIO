# `parquet` reader — Python + Julia track handoff

**Companion to [`conformance.md`](conformance.md) §3 ("Parquet decode notes") and
[`registries.md`](registries.md).** Status: a work order, not a contract — the
contract is conformance.md §3, which is already normative and language-neutral.

The Rust track ships a complete `parquet` reader
(`rust/src/format/parquet.rs`, `rust/tests/parquet_reader.rs`). Python and Julia
do not. This document says exactly what those two need so the decode contract
stays cross-binding, and why the conformance corpus case is deliberately not
committed yet.

## Why the branch stopped at one track

"In all three bindings" is the repo's standard (`1af53dc`, the shapefile
reader). It was not met here, and the honest reason is scope: the shapefile
commit touched 35 files across three language tracks plus the corpus, and the
Parquet type surface is wider than the shapefile one (every Arrow type, a null
policy, decimal-text parsing, projection pushdown). Rather than leave three
half-finished readers whose dumps disagree, the Rust track is complete and
correct and the other two are specified here.

**Nothing is half-done in the tree.** `parquet` is registered in the Rust
`FormatRegistry` only; the Python and Julia registries do not claim it, so a
document declaring `format: parquet` gets a clean "unknown format" registration
gap in those tracks rather than a reader that silently decodes something else.

## The contract to hit

Do not re-derive it. [`conformance.md`](conformance.md) §3 "Parquet decode
notes" pins, normatively:

- the Arrow → dtype table (including that the narrow/wide integer split is the
  **NetCDF reader's, verbatim** — restated so a MOVES `int32` ID column and a CF
  `int32` time axis cannot drift apart);
- temporal columns carried as their **raw integer**, unit and timezone unapplied;
- `Dictionary(K, V)` expanded to one `V` per row;
- nested/binary columns: an error when named in `variables`, not a field when not;
- a zero-row file being **typed, not absent**;
- the **null policy** (float → NaN; integer/string/boolean → an error naming the
  column and row; `null_int` / `null_string` as the only gates, with `null_int`
  reported back in `fill_value`);
- `float_columns`, including decimal **text** (trim, parse, blank → NaN,
  anything else unparseable an error naming column/row/text);
- projection pushdown by `variables`, and an absent name being an error that
  lists what is present.

`rust/src/format/parquet.rs` is the worked reference; its module docs restate the
same rules with the reasoning attached.

## Python

**Library:** `pyarrow` (`pyarrow.parquet`). It is the reference implementation
and the same project as the Rust `parquet` crate, so the type mapping is
literally the same enum on both sides — the reason parity here is cheap.

1. `pyproject.toml` — a new extra beside `shapefile = ["pyshp>=2.3"]`:
   ```toml
   # Parquet reader: pyarrow is the reference Apache Arrow/Parquet implementation
   # and the peer of the Rust `parquet` crate. Imported lazily so a base install
   # stays lean.
   parquet = ["pyarrow>=14"]
   ```
2. `earthsciio/readers.py` — a `ParquetReader` class in the shape of
   `ShapefileReader`: `NAME = "parquet"`, `FORMATS = ("parquet",)`,
   `EXTENSIONS = ("parquet", "parq", "pq")`, `open()` returning the path, and

   ```python
   def read_native(self, handle, variables=None, select=None, *,
                   float_columns=None, null_int=None, null_string=None, **_):
   ```

   with `import pyarrow.parquet as pq` **lazy inside the method**, raising the
   same `ImportError` shape as `ShapefileReader` (`pip install
   earthsciio[parquet]`). Push the projection down with
   `pq.read_table(path, columns=[...])` — `columns=None` reads everything.
   Resolve the requested names against `pq.ParquetFile(path).schema_arrow.names`
   **first**, so an unknown name is the reader's own error listing what is
   present rather than pyarrow's.
3. Add it to `register_format_readers` with `status="active"` and a `notes=`
   line, exactly as the other five.
4. `earthsciio/__init__.py` — export `ParquetReader`.
5. `tests/test_parquet_reader.py` — mirror `rust/tests/parquet_reader.rs` case
   for case. Write fixtures in the test with `pq.write_table`, not as committed
   blobs.

**Two Python-specific traps.**

- **Do not let pandas near this.** `Table.to_pandas()` silently promotes a
  nullable int64 column to `float64` (or to a pandas nullable dtype), which is
  precisely the substitution the null policy exists to forbid. Work off
  `pyarrow.Table` columns and `ChunkedArray.to_pylist()` / `.is_valid()`, and
  build the final arrays with `np.asarray(..., dtype=...)`.
- **`Utf8` → `str`, and nothing else.** `SCC` and other leading-zero codes must
  not become numbers. numpy will not do this to you, but a convenience
  conversion will.

## Julia

**Library:** `Parquet2.jl` — pure Julia, actively maintained, and reads the
Arrow logical types this contract is written against. (`Parquet.jl` is the older
package and is not recommended.)

Follow the **weakdep extension** pattern the Shapefile and TiffImages readers
already use, so a base `EarthSciIO` install does not pull the Parquet stack:

1. `julia/Project.toml` — `Parquet2` into `[weakdeps]`, `[compat]`, `[extras]`
   and the `test` target, plus
   `EarthSciIOParquet2Ext = "Parquet2"` in `[extensions]`.
2. `julia/src/readers.jl` — the `ParquetReader` struct, its registration, and
   **the whole decode CONTRACT** (dtype mapping, null policy, `float_columns`
   parsing) in a core `_assemble_parquet` helper. This is the split the
   shapefile reader already makes: the extension pulls raw columns out of the
   backend, the core owns the mapping, so the contract is shared with any future
   backend and is testable without the weakdep loaded.
3. `julia/ext/EarthSciIOParquet2Ext.jl` — the backend half: open the file, apply
   the column projection (`Parquet2.Dataset` then select), hand raw columns and
   their types to `_assemble_parquet`.
4. `julia/src/EarthSciIO.jl` — export, and the registry `notes` line.
5. `julia/test/test_parquet_reader.jl` + a line in `julia/test/runtests.jl`.

**Julia-specific trap.** Parquet2 surfaces a nullable column as
`Vector{Union{Missing,T}}`. `missing` is the null; do not let it reach the
native array. Apply the §3 policy at the boundary: `NaN` for floats, an error
naming the column and row for integers/strings/bools unless `null_int` /
`null_string` was declared.

## The conformance corpus case (do this last)

A corpus case is a **cross-language** artifact: `conformance/crosscheck.py`
compares the Python, Julia and Rust dumps of the same blob. Committing a
`parquet` case while only one track has a reader would fail the conformance job
for a gap the case does not describe, so it is reserved (see conformance.md,
"Committed cases"). Once **both** readers above exist:

1. Add a `parquet-table` case with `conformance/generate.py` as the template.
   Keep the blob small and make it earn its bytes — one column of each family
   that the unit tests cannot check cross-language, a real null in a float
   column, a `Dictionary` column, and a decimal-text column exercising
   `float_columns` through `reader_kwargs`.
2. `conformance/dumpers/dump_python.py` and `dumpers/dump_julia.jl` need the
   `float_columns`/`null_int`/`null_string` kwargs threaded through, the way
   `member_glob`/`skip_header_row` already are; `rust/examples/conformance_dump.rs`
   needs the same for the Rust side (it does **not** have them today).
3. `conformance/verify.py` needs an independent oracle for the case.
4. `spec/conformance.md`: add the row to "Committed cases" and delete the
   reserved-case paragraph.
5. `spec/registries.md` / `registries.json`: drop the "Rust; Py/Jl pending"
   qualifier from the `parquet` row.

## Also left undone in the Rust track

- **`rust/examples/conformance_dump.rs` does not thread parquet `reader_options`.**
  Harmless today (no corpus case reaches it), and step 2 above is where it lands.
- **Row-group / row-selection pushdown is not implemented, on purpose.** The
  reader could skip row groups, but `Provider::read_file` hands every whole-file
  reader `Selection::All` unconditionally, so nothing could reach it without
  editing `Provider` — which `format/mod.rs`'s own extensibility invariant
  forbids. If row pushdown is ever wanted for a whole-file reader, that is a
  Provider change and a spec change, not a reader change.
- **MSRV moved to 1.85** (arrow-rs's), from 1.74. CI builds on `stable`, so
  nothing enforces it either way; it is now a true statement instead of a false
  one.
