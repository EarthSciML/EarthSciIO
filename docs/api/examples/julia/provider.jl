# Read a cached file into native-grid arrays through a Provider.
using EarthSciIO

const FIXTURES = joinpath(@__DIR__, "..", "fixtures")
cache = Cache(; root = mktempdir(), offline = false) # hide

# region: provider ops=provider,native_dataset,native_field,csv_reader
url = "file://" * abspath(joinpath(FIXTURES, "stations.csv"))
p = const_provider(
    cache, url; format = "csv", source_loader = "stations",
    variables = ["value", "location_id"],
    reader_kwargs = (numeric_columns = ["latitude", "longitude", "value"],)
)
@assert is_const(p) && refresh_times(p) == Float64[]  # a CONST provider never refreshes
data = materialize(p)
@assert data isa NativeDataset
@assert Set(variable_names(data)) == Set(["location_id", "value"])
@assert data["value"].data == [152.3, 168.7, 98.1, 110.4]
# Digit-only text columns stay strings: nothing is inferred.
@assert data["location_id"].data == ["1", "1", "2", "2"]
# endregion
