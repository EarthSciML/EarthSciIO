"""Read a cached file into native-grid arrays through a Provider."""

import tempfile
from pathlib import Path

import numpy as np

from earthsciio import Cache, DataSource, NativeDataset, Provider

FIXTURES = Path(__file__).resolve().parent.parent / "fixtures"
cache = Cache(root=Path(tempfile.mkdtemp(prefix="earthsciio-example-")), offline=False)  # hide


# region: provider ops=provider,data_source,native_dataset,native_field,csv_reader
source = DataSource(
    "stations",
    "csv",  # the format-registry key that selects the reader
    (FIXTURES / "stations.csv").as_uri(),
    variables=["value", "location_id"],
    reader_kwargs={"numeric_columns": ["latitude", "longitude", "value"]},
)
data = Provider(source, cache).materialize()
assert isinstance(data, NativeDataset)
assert data.variable_names() == ["location_id", "value"]
assert data["value"].data.dtype == np.float64
assert list(data["value"].data) == [152.3, 168.7, 98.1, 110.4]
# Digit-only text columns stay strings: nothing is inferred.
assert data["location_id"].data == ["1", "1", "2", "2"]
# endregion
