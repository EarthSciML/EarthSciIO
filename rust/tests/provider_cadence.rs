//! The cadence-aware `Provider` over the committed ERA5 corpus fixture, fully
//! offline (acceptance: "Provider over the shared fixture returns native arrays
//! matching the tracks; CONST/DISCRETE correct; refresh_times() matches the
//! cadence"). All reads resolve from the Python-populated corpus cache — no
//! network, no per-language data tooling beyond the netcdf reader.

use std::sync::Arc;

use earthsciio::{ArrayData, Cache, DataLoader, Error, LoaderTemporal, NativeField, Provider};
use time::macros::datetime;
use time::Duration;

const ERA5_URL: &str = "https://data.earthsci.dev/era5/2018/11/20181108.nc";
// Resolves the file (day) anchor to ERA5_URL via `time` formatting.
const ERA5_TEMPLATE: &str = "https://data.earthsci.dev/era5/[year]/[month]/[year][month][day].nc";

fn corpus_cache() -> Arc<Cache> {
    let root =
        std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../conformance/corpus/cache");
    Arc::new(
        Cache::builder()
            .data_dir(root)
            .offline(true)
            .verify_on_read(true)
            .build()
            .expect("offline cache over the corpus"),
    )
}

fn f64s(field: &NativeField) -> &[f64] {
    match &field.data {
        ArrayData::F64(v) => v,
        other => panic!("expected f64 data, got {:?}", other.dtype()),
    }
}

// --- DISCRETE (era5 is hourly within a daily file) --------------------------

#[test]
fn discrete_refresh_times_match_the_hourly_cadence() {
    let window = (
        datetime!(2018-11-08 00:00:00 UTC),
        datetime!(2018-11-08 02:00:00 UTC),
    );
    let loader = DataLoader::new("era5", "netcdf", ERA5_TEMPLATE).temporal(LoaderTemporal::new(
        datetime!(2018-11-08 00:00:00 UTC),
        Duration::hours(1),
        Duration::days(1),
    ));
    let provider = Provider::new(loader, corpus_cache(), Some(window)).unwrap();

    let times = provider.refresh_times();
    // Two hourly tstops in [00:00, 02:00): 00:00 and 01:00.
    assert_eq!(times.len(), 2, "hourly cadence over a 2-hour window");
    assert_eq!(
        times[0],
        datetime!(2018-11-08 00:00:00 UTC).unix_timestamp() as f64
    );
    assert_eq!(times[1] - times[0], 3600.0, "one-hour cadence step");
}

#[test]
fn discrete_refresh_selects_the_right_record_at_each_boundary() {
    let window = (
        datetime!(2018-11-08 00:00:00 UTC),
        datetime!(2018-11-08 02:00:00 UTC),
    );
    let loader = DataLoader::new("era5", "netcdf", ERA5_TEMPLATE).temporal(LoaderTemporal::new(
        datetime!(2018-11-08 00:00:00 UTC),
        Duration::hours(1),
        Duration::days(1),
    ));
    let mut provider = Provider::new(loader, corpus_cache(), Some(window)).unwrap();

    // materialize() primes the buffer at the first anchor (record 0).
    let buf0 = provider.materialize().unwrap();
    let t2m0 = &buf0["t2m"];
    assert_eq!(
        t2m0.dims,
        vec!["latitude".to_string(), "longitude".to_string()]
    );
    assert_eq!(
        t2m0.shape,
        vec![3, 3],
        "a single hourly slice, time axis dropped"
    );
    assert_eq!(f64s(t2m0)[0], 282.5); // record-0 first cell
    assert_eq!(f64s(t2m0)[8], 284.5); // record-0 last cell (not masked)

    // Within the same hour: no change.
    assert!(
        provider
            .refresh(datetime!(2018-11-08 00:30:00 UTC))
            .unwrap()
            .is_none(),
        "t inside the same cadence interval must not refresh"
    );

    // Next hour: record 1, which carries the masked (NaN) cell.
    let buf1 = provider
        .refresh(datetime!(2018-11-08 01:00:00 UTC))
        .unwrap()
        .expect("crossing the cadence boundary refreshes");
    let t2m1 = &buf1["t2m"];
    assert_eq!(t2m1.shape, vec![3, 3]);
    assert_eq!(f64s(t2m1)[0], 282.6); // record-1 first cell
    assert!(
        f64s(t2m1)[8].is_nan(),
        "record-1 last cell is _FillValue -> NaN"
    );

    // Re-asking for the same hour is idempotent.
    assert!(provider
        .refresh(datetime!(2018-11-08 01:00:00 UTC))
        .unwrap()
        .is_none());

    // Native coordinates are exposed on the loader's grid.
    let coords = provider.coords();
    assert_eq!(f64s(&coords["latitude"].field), &[40.0, 39.5, 39.0]);
    assert_eq!(f64s(&coords["longitude"].field), &[-122.0, -121.5, -121.0]);
    assert_eq!(
        coords["time"].units.as_deref(),
        Some("hours since 2018-11-08 00:00:00"),
        "time axis carries CF units verbatim (calendar decoding stays in ESS)"
    );
}

#[test]
fn discrete_prefetch_warms_the_window_offline() {
    let window = (
        datetime!(2018-11-08 00:00:00 UTC),
        datetime!(2018-11-08 02:00:00 UTC),
    );
    let loader = DataLoader::new("era5", "netcdf", ERA5_TEMPLATE).temporal(LoaderTemporal::new(
        datetime!(2018-11-08 00:00:00 UTC),
        Duration::hours(1),
        Duration::days(1),
    ));
    let mut provider = Provider::new(loader, corpus_cache(), Some(window)).unwrap();
    // The day's file is in the corpus → prefetch resolves it with no network.
    provider
        .prefetch(window)
        .expect("prefetch the window from the corpus");
}

// --- CONST (no temporal: read once, never refresh) --------------------------

#[test]
fn const_materialize_reads_whole_file_and_never_refreshes() {
    // A loader with no temporal block is CONST. Pointed at the same ERA5 blob via
    // a literal URL, materialize() returns the full (un-sliced) native arrays.
    let loader = DataLoader::new("era5_static", "netcdf", ERA5_URL);
    let mut provider = Provider::new(loader, corpus_cache(), None).unwrap();

    let buf = provider.materialize().unwrap();
    let t2m = &buf["t2m"];
    assert_eq!(
        t2m.dims,
        vec![
            "time".to_string(),
            "latitude".to_string(),
            "longitude".to_string()
        ]
    );
    assert_eq!(t2m.shape, vec![2, 3, 3], "CONST keeps the full file shape");
    assert_eq!(f64s(t2m)[0], 282.5);

    // CONST: refresh is a no-op and the cadence schedule is empty.
    assert!(provider
        .refresh(datetime!(2018-11-08 01:00:00 UTC))
        .unwrap()
        .is_none());
    assert!(provider.refresh_times().is_empty());
}

// --- registry plumbing ------------------------------------------------------

#[test]
fn unknown_format_is_a_clean_error() {
    let loader = DataLoader::new("mystery", "zarr", ERA5_URL); // no zarr reader yet
                                                               // (Provider holds trait objects and isn't Debug, so match rather than unwrap_err.)
    match Provider::new(loader, corpus_cache(), None) {
        Err(Error::UnknownFormat { name }) => assert_eq!(name, "zarr"),
        Err(other) => panic!("expected UnknownFormat, got {other}"),
        Ok(_) => panic!("expected UnknownFormat error for an unregistered format"),
    }
}
