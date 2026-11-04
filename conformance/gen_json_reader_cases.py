"""Writes conformance/corpus/scenarios/json-reader.json (run: python3
conformance/gen_json_reader_cases.py conformance/corpus/scenarios/json-reader.json).
The expected values are written out by hand below; no reader computed them."""

import json
import sys

NaN = None  # NaN is written as null in the fixture


def ok(id, doc, expected_len, fields, options=None, variables=None, note=""):
    c = {"id": id, "document": doc}
    if options:
        c["options"] = options
    if variables is not None:
        c["variables"] = variables
    c["expected"] = {"length": expected_len, "fields": fields}
    if note:
        c["note"] = note
    return c


def err(id, doc, code, options=None, variables=None, note=""):
    c = {"id": id, "document": doc}
    if options:
        c["options"] = options
    if variables is not None:
        c["variables"] = variables
    c["error"] = code
    if note:
        c["note"] = note
    return c


def F(*d):
    return {"dtype": "float64", "data": list(d)}


def S(*d):
    return {"dtype": "string", "data": list(d)}


def B(*d):
    return {"dtype": "bool", "data": list(d)}


cases = [
    ok(
        "root-array",
        '[{"a": 1, "b": "x", "c": true}, {"a": 2.5, "b": "y", "c": false}]',
        2,
        {"a": F(1.0, 2.5), "b": S("x", "y"), "c": B(True, False)},
        note="The root is the records array; one field per kind.",
    ),
    ok(
        "station-discovery",
        json.dumps(
            {
                "meta": {"name": "openaq-api", "found": 2},
                "results": [
                    {
                        "id": 101,
                        "name": "Site A",
                        "locality": None,
                        "isMobile": False,
                        "coordinates": {"latitude": 39.76, "longitude": -121.62},
                        "sensors": [{"id": 1, "parameter": "pm25"}],
                    },
                    {
                        "id": 102,
                        "name": "Site B",
                        "locality": "Chico",
                        "isMobile": False,
                        "coordinates": {"latitude": 39.5, "longitude": -121.5},
                        "sensors": [],
                    },
                ],
            }
        ),
        2,
        {
            "id": F(101.0, 102.0),
            "name": S("Site A", "Site B"),
            "locality": S("", "Chico"),
            "isMobile": B(False, False),
            "coordinates.latitude": F(39.76, 39.5),
            "coordinates.longitude": F(-121.62, -121.5),
        },
        options={"records_path": ["results"]},
        variables=[
            "id",
            "name",
            "locality",
            "isMobile",
            "coordinates.latitude",
            "coordinates.longitude",
        ],
        note="records_path reaches the array; nested objects flatten with '.'; the array-valued "
        "`sensors` is not decoded because `variables` leaves it out.",
    ),
    ok(
        "deep-records-path",
        '{"data": {"items": [{"v": 1}, {"v": 2}]}}',
        2,
        {"v": F(1.0, 2.0)},
        options={"records_path": ["data", "items"]},
    ),
    ok(
        "missing-and-null",
        '[{"a": 1, "s": "p"}, {"s": null}, {"a": null}]',
        3,
        {"a": F(1.0, NaN, NaN), "s": S("p", "", "")},
        note='An absent member is null; null is NaN in a number field and "" in a string field.',
    ),
    ok("all-null", '[{"z": null}, {"z": null}]', 2, {"z": F(NaN, NaN)}),
    ok(
        "numeric-columns",
        '[{"v": "12.5", "w": 1}, {"v": " 3e2 "}, {"v": 7}, {"v": null}, {"v": "-.5"}]',
        5,
        {"v": F(12.5, 300.0, 7.0, NaN, -0.5), "w": F(1.0, NaN, NaN, NaN, NaN)},
        options={"numeric_columns": ["v", "not_a_field"]},
        note="A numeric_columns string cell is parsed after trimming ASCII whitespace; a name no record has is ignored, as in csv.",
    ),
    ok(
        "integers-round-to-nearest",
        '[{"n": 9007199254740993}, {"n": 123456789012345678901234567890}, {"n": -0}, {"n": 1e-400}]',
        4,
        {"n": F(9007199254740992.0, 1.2345678901234568e29, 0.0, 0.0)},
        note="2^53+1 rounds to 2^53 (ties to even); integers past int64 round too; 1e-400 underflows to 0.",
    ),
    ok("empty-array", "[]", 0, {}),
    ok(
        "empty-object-member",
        '[{"a": {}, "b": 1}]',
        1,
        {"b": F(1.0)},
        note="An empty nested object contributes no field.",
    ),
    ok("duplicate-member-last-wins", '[{"a": 1, "a": 2}]', 1, {"a": F(2.0)}),
    ok(
        "unicode",
        '[{"s": "caf\\u00e9 \\ud83d\\ude00", "t": "über"}]',
        1,
        {"s": S("café \U0001f600"), "t": S("über")},
    ),
    ok(
        "variables-projection",
        '[{"a": 1, "b": [1, 2], "c": "x"}]',
        1,
        {"c": S("x")},
        variables=["c"],
        note="Fields left out of `variables` are not decoded, so `b` may be an array.",
    ),
    err("parse-truncated", '[{"a": 1}', "json_parse"),
    err("parse-nan-literal", '[{"a": NaN}]', "json_parse", note="NaN is not a JSON literal."),
    err("parse-infinity-literal", '[{"a": -Infinity}]', "json_parse"),
    err(
        "parse-overflow",
        '[{"a": 1e400}]',
        "json_parse",
        note="A number past the float64 range is an error, not infinity.",
    ),
    err("parse-trailing", "[] x", "json_parse"),
    err("parse-trailing-comma", '[{"a": 1},]', "json_parse"),
    err("root-object-without-path", '{"results": []}', "json_records_path"),
    err(
        "records-path-missing-key",
        '{"results": []}',
        "json_records_path",
        options={"records_path": ["data"]},
    ),
    err(
        "records-path-through-array",
        '{"results": [1]}',
        "json_records_path",
        options={"records_path": ["results", "x"]},
    ),
    err(
        "records-path-not-array",
        '{"results": {"a": 1}}',
        "json_records_path",
        options={"records_path": ["results"]},
    ),
    err("record-not-object", '[{"a": 1}, 2]', "json_record_not_object"),
    err("nested-array", '[{"a": [1, 2]}]', "json_nested_array"),
    err("mixed-number-string", '[{"a": 1}, {"a": "x"}]', "json_mixed_types"),
    err("mixed-number-bool", '[{"a": 1}, {"a": true}]', "json_mixed_types"),
    err("null-bool", '[{"a": true}, {"a": null}]', "json_null_bool"),
    err("absent-bool", '[{"a": true}, {}]', "json_null_bool"),
    err("leaf-and-object", '[{"a": 1}, {"a": {"b": 2}}]', "json_field_conflict"),
    err("flatten-collision", '[{"a.b": 1, "a": {"b": 2}}]', "json_field_conflict"),
    err(
        "numeric-text-word",
        '[{"v": "abc"}]',
        "json_numeric_text",
        options={"numeric_columns": ["v"]},
    ),
    err(
        "numeric-text-blank", '[{"v": ""}]', "json_numeric_text", options={"numeric_columns": ["v"]}
    ),
    err(
        "numeric-text-nan",
        '[{"v": "nan"}]',
        "json_numeric_text",
        options={"numeric_columns": ["v"]},
    ),
    err(
        "numeric-text-underscore",
        '[{"v": "1_000"}]',
        "json_numeric_text",
        options={"numeric_columns": ["v"]},
    ),
    err("numeric-bool", '[{"v": true}]', "json_numeric_text", options={"numeric_columns": ["v"]}),
    err("variables-missing", '[{"a": 1}]', "json_variables_missing", variables=["b"]),
    err("variables-on-empty", "[]", "json_variables_missing", variables=["a"]),
    err(
        "records-path-not-list",
        '{"results": []}',
        "json_option",
        options={"records_path": "results"},
    ),
    err("numeric-columns-not-list", '[{"v": 1}]', "json_option", options={"numeric_columns": "v"}),
    err(
        "records-path-not-list-int",
        '[{"v": 1}]',
        "json_option",
        options={"records_path": 5},
        note="An option that is not an array is json_option, whatever it is.",
    ),
    err(
        "records-path-object",
        '{"results": [{"v": 1}]}',
        "json_option",
        options={"records_path": {"results": 1}},
        note="An object is not an array of strings (its keys are not the path).",
    ),
    # --- parse errors anywhere in the document, decoded or not ---------------
    ok(
        "signed-zero-is-positive",
        '[{"n": -0, "v": "-0.0"}, {"n": -0.0, "v": "-0"}, {"n": 0, "v": "+0e5"}]',
        3,
        {"n": F(0.0, 0.0, 0.0), "v": F(0.0, 0.0, 0.0)},
        options={"numeric_columns": ["v"]},
        note="A zero is +0.0 whatever its sign, an integer, a float or a numeric_columns string "
        "(the harnesses compare bitwise).",
    ),
    err(
        "out-of-range-float-undecoded",
        '[{"a": 1, "big": 1e400}]',
        "json_parse",
        variables=["a"],
        note="A number outside the float64 range is a parse error even in a field left out of "
        "`variables`.",
    ),
    err(
        "out-of-range-integer-undecoded",
        '[{"a": 1, "big": 1' + "0" * 400 + "}]",
        "json_parse",
        variables=["a"],
    ),
    err(
        "out-of-range-negative-in-array",
        '[{"a": 1, "xs": [1, -1e309]}]',
        "json_parse",
        variables=["a"],
    ),
    err(
        "lone-high-surrogate",
        '[{"s": "\\ud800"}]',
        "json_parse",
        note="An unpaired UTF-16 surrogate escape is undefined text: refused, not passed on.",
    ),
    err(
        "lone-low-surrogate-undecoded",
        '[{"a": 1, "s": "x\\udc00y"}]',
        "json_parse",
        variables=["a"],
    ),
    err("lone-surrogate-in-member-name", '[{"a": 1, "\\ud800": 2}]', "json_parse", variables=["a"]),
    ok(
        "surrogate-pair",
        '[{"s": "\\ud83d\\ude00"}]',
        1,
        {"s": S("\U0001f600")},
        note="A high+low pair is one code point.",
    ),
    ok(
        "nesting-127-levels",
        '[{"a": 1, "d": ' + "[" * 125 + "]" * 125 + "}]",
        1,
        {"a": F(1.0)},
        variables=["a"],
        note="Root array (1) + record (2) + 125 arrays: 127 levels, the deepest allowed.",
    ),
    err(
        "nesting-128-levels",
        '[{"a": 1, "d": ' + "[" * 126 + "]" * 126 + "}]",
        "json_parse",
        variables=["a"],
        note="One level past 127 is a parse error in every track (serde_json's limit).",
    ),
    err(
        "nesting-very-deep",
        "[" * 5000 + "]" * 5000,
        "json_parse",
        note="Far past any recursion limit: still json_parse, never a stack error.",
    ),
]
ids = [c["id"] for c in cases]
assert len(ids) == len(set(ids))
out = {
    "schema": "earthsciio/scenario/v1",
    "id": "json-reader",
    "description": "The json points reader (spec/registries.md §2, 'JSON points'): each case is a JSON "
    "document as text, the reader options and `variables` to decode it with, and either "
    "the expected fields (NaN written as null) or the error code the decode must raise. "
    "Run by the Python, Julia and Rust test suites. Written by hand (the generator is "
    "conformance/gen_json_reader_cases.py); no reader computed these values.",
    "cases": cases,
}
with open(sys.argv[1], "w") as fh:
    json.dump(out, fh, indent=2, ensure_ascii=False)
    fh.write("\n")
