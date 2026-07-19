# Emissions.jl FF10 raw-parse cross-check (external oracle, MANUAL — not CI).
#
# EarthSciIO must NOT depend on Emissions.jl, so this is run by hand. It reads the
# SAME committed conformance fixture (conformance/corpus/.../ff10_point.csv) with
# Emissions.jl's own read path — `CSV.read(path, DataFrame; header=false,
# comment="#", silencewarnings=true)` then `rename!(df, FF10_POINT_COLUMNS)` (the
# 77 names copied verbatim from `Emissions.jl/src/ff10.jl` `FF10_POINT_COLUMNS`) —
# and asserts the RAW column values (the DataFrame BEFORE the `FF10PointDataFrame`
# constructor's transforms) equal the fixture's `expected.variables`. This proves
# the EarthSciIO `ff10` reader's SCHEMA + RAW PARSE match Emissions.jl's,
# independent of the deferred `.esm` conversions.
#
# Naming alias (USER OVERRIDE): EarthSciIO / SMOKE names the first two columns
# COUNTRY_CD and REGION_CD; Emissions.jl names them COUNTRY and FIPS. They carry
# IDENTICAL values — only the names differ — so the schema is aligned POSITIONALLY
# (COUNTRY_CD ↔ COUNTRY, REGION_CD ↔ FIPS). The 77 names below are the SMOKE names;
# swapping in COUNTRY/FIPS for columns 1–2 gives Emissions.jl's exact list.
#
# Raw-parse surface note: EarthSciIO reads ALL id/code columns as String so
# leading-zero codes survive verbatim (REGION_CD "01001", ZIPCODE "00000", SCC).
# Emissions.jl's DEFAULT `CSV.read` inference instead reads FIPS as an Int (1001)
# and restores the leading zeros DOWNSTREAM via `transform_fips!`'s
# `lpad(string(fips), 5, '0')` — a transform this reader defers. The reader-only
# parity surface is therefore the RAW FIELD TEXT, captured here with `types=String`
# (which is what EarthSciIO returns and what `transform_fips!` reproduces).
#
# Run:  julia --project=<env-with-CSV+DataFrames> conformance/ff10_oracle_emissions.jl

using CSV, DataFrames
import JSON

# 77 FF10 point column names — copied from Emissions.jl `src/ff10.jl`
# `FF10_POINT_COLUMNS`, with SMOKE names COUNTRY_CD/REGION_CD for cols 1–2
# (Emissions.jl: COUNTRY/FIPS; identical values, positional alias).
const FF10_POINT_COLUMNS = String[
    "COUNTRY_CD", "REGION_CD", "TRIBAL_CODE", "FACILITY_ID",
    "UNIT_ID", "REL_POINT_ID", "PROCESS_ID", "AGY_FACILITY_ID",
    "AGY_UNIT_ID", "AGY_REL_POINT_ID", "AGY_PROCESS_ID", "SCC",
    "POLID", "ANN_VALUE", "ANN_PCT_RED", "FACILITY_NAME",
    "ERPTYPE", "STKHGT", "STKDIAM", "STKTEMP",
    "STKFLOW", "STKVEL", "NAICS", "LONGITUDE",
    "LATITUDE", "LL_DATUM", "HORIZ_COLL_MTHD", "DESIGN_CAPACITY",
    "DESIGN_CAPACITY_UNITS", "REG_CODES", "FAC_SOURCE_TYPE", "UNIT_TYPE_CODE",
    "CONTROL_IDS", "CONTROL_MEASURES", "CURRENT_COST", "CUMULATIVE_COST",
    "PROJECTION_FACTOR", "SUBMITTER_FAC_ID", "CALC_METHOD", "DATA_SET_ID",
    "FACIL_CATEGORY_CODE", "ORIS_FACILITY_CODE", "ORIS_BOILER_ID", "IPM_YN",
    "CALC_YEAR", "DATE_UPDATED", "FUG_HEIGHT", "FUG_WIDTH_XDIM",
    "FUG_LENGTH_YDIM", "FUG_ANGLE", "ZIPCODE", "ANNUAL_AVG_HOURS_PER_YEAR",
    "JAN_VALUE", "FEB_VALUE", "MAR_VALUE", "APR_VALUE",
    "MAY_VALUE", "JUN_VALUE", "JUL_VALUE", "AUG_VALUE",
    "SEP_VALUE", "OCT_VALUE", "NOV_VALUE", "DEC_VALUE",
    "JAN_PCTRED", "FEB_PCTRED", "MAR_PCTRED", "APR_PCTRED",
    "MAY_PCTRED", "JUN_PCTRED", "JUL_PCTRED", "AUG_PCTRED",
    "SEP_PCTRED", "OCT_PCTRED", "NOV_PCTRED", "DEC_PCTRED",
    "COMMENT",
]

function main()
    corpus = normpath(joinpath(@__DIR__, "corpus"))
    case = JSON.parsefile(joinpath(corpus, "cases", "ff10-point-slice.json"))
    blob = joinpath(corpus, case["blob_path"])

    # Emissions.jl's read path: no header row, '#' comment block skipped. `types =
    # String` captures the RAW field text (see the raw-parse note above).
    df = CSV.read(blob, DataFrame; header = false, comment = "#",
                  types = String, silencewarnings = true)
    rename!(df, FF10_POINT_COLUMNS)

    @assert size(df, 2) == 77 "expected 77 columns, got $(size(df,2))"
    exp = case["expected"]["variables"]
    nrows = size(df, 1)
    println("Emissions.jl raw-parse oracle: $(size(df,2)) columns, $nrows rows")

    numeric = Set(String.(case["decode"]["numeric_columns"]))
    mism = 0
    for name in FF10_POINT_COLUMNS
        col = df[!, name]
        want = exp[name]["data"]
        if name in numeric
            got = Float64[ismissing(v) ? NaN : parse(Float64, v) for v in col]
            w = Float64[v === nothing ? NaN : Float64(v) for v in want]
            ok = all((isnan.(got) .& isnan.(w)) .| (got .== w))
        else
            got = String[ismissing(v) ? "" : String(v) for v in col]
            w = String[String(v) for v in want]
            ok = got == w
        end
        ok || (mism += 1; println("  MISMATCH in $name: got=$got want=$want"))
    end

    if mism == 0
        println("PASS: EarthSciIO ff10 schema + raw parse == Emissions.jl oracle ",
                "(all 77 columns, $nrows rows; COUNTRY_CD↔COUNTRY, REGION_CD↔FIPS).")
    else
        println("FAIL: $mism column(s) mismatched")
        exit(1)
    end
end

main()
