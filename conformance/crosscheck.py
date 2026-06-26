#!/usr/bin/env python3
"""Cross-language native-array comparator — the gate of the conformance harness.

This is the part of ``esio-9nb.9`` that **asserts native-array equality across
Python / Julia / Rust**. It consumes the per-track dumps emitted by the three
provider dumpers (``conformance/dumpers/dump_python.py``,
``conformance/dumpers/dump_julia.jl``,
``rust/examples/conformance_dump.rs`` — schema ``earthsciio/native-dump/v1``) and,
for every committed corpus case, checks:

  * **vs the oracle** — each decoding track's arrays equal the corpus
    ``expected`` arrays (the Python/xarray oracle baked into the fixtures);
  * **pairwise** — every pair of tracks that decoded the case produced *equal*
    native arrays (same variable/coord set, dtype, dims, shape, values);
  * **coverage** — a track that registers a reader for a case's format MUST have
    decoded it (a silently-dropped case is a failure); a track with no reader for
    that format may skip, and the gap is logged, never hidden.

Tolerance (``spec/conformance.md`` §4, identical to ``conformance/verify.py``):

  * CF-decoded / unit-affected floats: ``|a-b| <= atol + rtol*|b|`` with
    ``atol = 1e-6``, ``rtol = 1e-9`` (libraries differ at the ULP level);
  * raw integer reads and strings: compared **exactly**;
  * a masked / ``_FillValue`` cell is ``null`` in a dump and compares equal only
    to ``null`` (the NaN/fill mask must match element-for-element).

Global gates (all must hold for exit 0):

  * every case is decoded by at least one track (a 0-decoder case ⇒ broken corpus);
  * at least one case is decoded by **all three** tracks (so the harness actually
    demonstrates three-way equality, not just pairwise on disjoint subsets);
  * no oracle mismatch, no pairwise mismatch, no missing-but-claimed reader.

Usage:  python3 conformance/crosscheck.py DUMP.json [DUMP.json ...]
        # each dump self-identifies its language; ≥2 dumps required.
Spec:   ../spec/conformance.md ; conformance/CROSSLANG.md
"""

from __future__ import annotations

import json
import pathlib
import sys
from typing import Any, Dict, List, Optional, Tuple

ATOL = 1e-6
RTOL = 1e-9

HERE = pathlib.Path(__file__).resolve().parent
CORPUS = HERE / "corpus"

ALL_LANGS = ("python", "julia", "rust")


# --------------------------------------------------------------------------- #
# Flatten + value comparison (mirror conformance/verify.py semantics).
# --------------------------------------------------------------------------- #


def _flat(x: Any) -> List[Any]:
    """Row-major flatten of a nested list; scalars/None pass through."""
    out: List[Any] = []

    def rec(y: Any) -> None:
        if isinstance(y, list):
            for e in y:
                rec(e)
        else:
            out.append(y)

    rec(x)
    return out


def _values_equal(got: List[Any], exp: List[Any], dtype: str) -> Optional[str]:
    """Compare two flat value lists by dtype; ``None`` ⇒ equal, else an error str.

    ``None`` encodes a fill/NaN cell and matches only ``None``. Strings and
    integer reads compare exactly; floats within the documented tolerance.
    """
    if len(got) != len(exp):
        return f"length {len(got)} != {len(exp)}"
    string = dtype == "string"
    integral = dtype in ("int32", "int64")
    for i, (g, e) in enumerate(zip(got, exp)):
        gnull, enull = g is None, e is None
        if gnull != enull:
            return f"fill/NaN mask differs at [{i}]: {g!r} vs {e!r}"
        if gnull:
            continue
        if string:
            if str(g) != str(e):
                return f"string differs at [{i}]: {g!r} != {e!r}"
        elif integral:
            if int(g) != int(e):
                return f"int differs at [{i}]: {g} != {e}"
        else:
            if abs(float(g) - float(e)) > ATOL + RTOL * abs(float(e)):
                return (
                    f"value differs at [{i}]: {g} != {e} "
                    f"(beyond atol={ATOL:g}, rtol={RTOL:g})"
                )
    return None


# --------------------------------------------------------------------------- #
# Oracle (the corpus `expected` arrays) — pure-Python, no numpy needed.
# --------------------------------------------------------------------------- #


def load_oracle() -> Dict[str, Dict[str, Any]]:
    """``{case_id: {format, variables{name->field}, coords{name->field}}}`` from
    the committed corpus. A field is ``{dtype, dims?, shape?, data(flat),
    units?, calendar?}`` — coords carry no dims/shape in the corpus."""
    index = json.loads((CORPUS / "cases.json").read_text())
    oracle: Dict[str, Dict[str, Any]] = {}
    for entry in index["cases"]:
        case = json.loads((CORPUS / entry["file"]).read_text())
        variables = {}
        for name, spec in case["expected"]["variables"].items():
            variables[name] = {
                "dtype": spec["dtype"],
                "dims": spec.get("dims"),
                "shape": spec.get("shape"),
                "data": _flat(spec["data"]),
            }
        coords = {}
        for name, spec in case["expected"].get("coords", {}).items():
            field = {"dtype": spec["dtype"], "data": _flat(spec["data"])}
            for k in ("units", "calendar"):
                if k in spec:
                    field[k] = spec[k]
            coords[name] = field
        oracle[case["id"]] = {
            "format": case["format"],
            "variables": variables,
            "coords": coords,
        }
    return oracle


# --------------------------------------------------------------------------- #
# Field comparisons.
# --------------------------------------------------------------------------- #


def _cmp_against_oracle(got: Dict[str, Any], exp: Dict[str, Any], label: str) -> List[str]:
    """One decoded dump field vs an oracle field. dims/shape checked only when the
    oracle pins them (corpus variables do; coords don't)."""
    errs: List[str] = []
    if got["dtype"] != exp["dtype"]:
        errs.append(f"{label}: dtype {got['dtype']} != oracle {exp['dtype']}")
    if exp.get("dims") is not None and got.get("dims") != exp["dims"]:
        errs.append(f"{label}: dims {got.get('dims')} != oracle {exp['dims']}")
    if exp.get("shape") is not None and got.get("shape") != exp["shape"]:
        errs.append(f"{label}: shape {got.get('shape')} != oracle {exp['shape']}")
    for k in ("units", "calendar"):
        if k in exp and str(got.get(k)) != str(exp[k]):
            errs.append(f"{label}: {k} {got.get(k)!r} != oracle {exp[k]!r}")
    verr = _values_equal(got["data"], exp["data"], exp["dtype"])
    if verr:
        errs.append(f"{label}: {verr}")
    return errs


def _cmp_two_fields(a: Dict[str, Any], b: Dict[str, Any], label: str) -> List[str]:
    """Two decoded dump fields (full structural equality, both carry dims/shape)."""
    errs: List[str] = []
    if a["dtype"] != b["dtype"]:
        errs.append(f"{label}: dtype {a['dtype']} != {b['dtype']}")
    if a.get("dims") != b.get("dims"):
        errs.append(f"{label}: dims {a.get('dims')} != {b.get('dims')}")
    if a.get("shape") != b.get("shape"):
        errs.append(f"{label}: shape {a.get('shape')} != {b.get('shape')}")
    for k in ("units", "calendar"):
        if a.get(k) != b.get(k):
            errs.append(f"{label}: {k} {a.get(k)!r} != {b.get(k)!r}")
    verr = _values_equal(a["data"], b["data"], a["dtype"])
    if verr:
        errs.append(f"{label}: {verr}")
    return errs


def _fields(case_entry: Dict[str, Any]) -> Dict[str, Dict[str, Any]]:
    """Flatten a decoded dump case to ``{'<kind>:<name>': field}`` so variables and
    coords compare under one namespace."""
    out: Dict[str, Dict[str, Any]] = {}
    for name, f in case_entry.get("variables", {}).items():
        out[f"var:{name}"] = f
    for name, f in case_entry.get("coords", {}).items():
        out[f"coord:{name}"] = f
    return out


# --------------------------------------------------------------------------- #
# Driver.
# --------------------------------------------------------------------------- #


def _decoders(dumps: Dict[str, Any], case_id: str) -> Tuple[List[str], List[str]]:
    """``(decoded_langs, skipped_langs)`` for a case, in canonical lang order."""
    decoded, skipped = [], []
    for lang in ALL_LANGS:
        d = dumps.get(lang)
        if d is None:
            continue
        entry = d["cases"].get(case_id)
        if entry is None:
            continue
        if entry.get("status") == "decoded":
            decoded.append(lang)
        else:
            skipped.append(lang)
    return decoded, skipped


def crosscheck(dump_paths: List[str]) -> int:
    dumps: Dict[str, Any] = {}
    for p in dump_paths:
        d = json.loads(pathlib.Path(p).read_text())
        lang = d["language"]
        if d.get("schema") != "earthsciio/native-dump/v1":
            print(f"ERROR: {p}: unexpected schema {d.get('schema')!r}")
            return 1
        dumps[lang] = d

    present = [l for l in ALL_LANGS if l in dumps]
    if len(present) < 2:
        print(f"ERROR: need ≥2 language dumps to cross-check, got {present}")
        return 1

    oracle = load_oracle()
    print("=== Cross-language conformance: native-array equality (esio-9nb.9) ===")
    print(f"corpus:    {CORPUS}")
    print(f"dumps:     {', '.join(present)}")
    print(
        f"tolerance: atol={ATOL:g}, rtol={RTOL:g} (CF-decoded floats); "
        f"exact (int/string); null↔NaN  [spec/conformance.md §4]"
    )
    print()

    failures = 0
    all_three_cases: List[str] = []

    for case_id, ocase in oracle.items():
        fmt = ocase["format"]
        decoded, skipped = _decoders(dumps, case_id)
        print(f"case {case_id} ({fmt})")

        if not decoded:
            print("  FAIL: no track decoded this case (broken corpus or all readers missing)")
            failures += 1
            print()
            continue

        skip_notes = []
        for lang in skipped:
            reason = dumps[lang]["cases"][case_id].get("reason", "skipped")
            skip_notes.append(f"{lang} [{reason}]")
        print(f"  decoders: {', '.join(decoded)}" + (f"   skipped: {', '.join(skip_notes)}" if skip_notes else ""))

        # Coverage: a track that registers a reader for this format MUST decode it.
        for lang in present:
            readers = dumps[lang].get("readers", [])
            entry = dumps[lang]["cases"].get(case_id, {})
            if fmt in readers and entry.get("status") != "decoded":
                print(f"  FAIL: {lang} registers a '{fmt}' reader but did not decode this case")
                failures += 1

        # Each decoding track vs the oracle. Also pins the variable/coord set.
        oracle_fields = {f"var:{n}": v for n, v in ocase["variables"].items()}
        oracle_fields.update({f"coord:{n}": v for n, v in ocase["coords"].items()})
        for lang in decoded:
            fields = _fields(dumps[lang]["cases"][case_id])
            errs: List[str] = []
            for key, exp in oracle_fields.items():
                if key not in fields:
                    errs.append(f"{key}: missing from {lang} output")
                    continue
                errs += _cmp_against_oracle(fields[key], exp, key)
            if errs:
                failures += 1
                print(f"  FAIL vs oracle: {lang}")
                for e in errs:
                    print(f"      - {e}")
            else:
                print(f"  vs oracle: {lang} OK ({len(oracle_fields)} fields)")

        # Pairwise structural equality between every pair that decoded.
        for i in range(len(decoded)):
            for j in range(i + 1, len(decoded)):
                la, lb = decoded[i], decoded[j]
                fa, fb = _fields(dumps[la]["cases"][case_id]), _fields(dumps[lb]["cases"][case_id])
                errs = []
                if set(fa) != set(fb):
                    only_a = sorted(set(fa) - set(fb))
                    only_b = sorted(set(fb) - set(fa))
                    errs.append(f"field set differs: only in {la}={only_a}, only in {lb}={only_b}")
                for key in sorted(set(fa) & set(fb)):
                    errs += _cmp_two_fields(fa[key], fb[key], key)
                if errs:
                    failures += 1
                    print(f"  FAIL pairwise: {la} vs {lb}")
                    for e in errs:
                        print(f"      - {e}")
                else:
                    print(f"  pairwise: {la} = {lb} OK")

        if len(decoded) == 1:
            print(f"  note: only {decoded[0]} decoded this case — no cross-language check possible")
        if all(l in decoded for l in ALL_LANGS):
            all_three_cases.append(case_id)
        print()

    # Global gate: the harness must prove three-way equality on ≥1 case.
    if not all_three_cases:
        print("FAIL: no case was decoded by all three tracks — three-way equality not demonstrated")
        failures += 1

    print("=== summary ===")
    print(f"cases:                 {len(oracle)}")
    print(f"three-way-equality on: {', '.join(all_three_cases) if all_three_cases else '(none)'}")
    if failures:
        print(f"RESULT: FAIL ({failures} problem(s))")
        return 1
    print("RESULT: PASS — all decoded providers agree with the oracle and pairwise; "
          "three-way equality demonstrated.")
    return 0


def main(argv: List[str]) -> int:
    if len(argv) < 2:
        print(__doc__)
        print("error: provide ≥2 dump files", file=sys.stderr)
        return 2
    return crosscheck(argv[1:])


if __name__ == "__main__":
    sys.exit(main(sys.argv))
