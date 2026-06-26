//! ERA5 pressure-level request mapping for the Copernicus CDS API v1, ported
//! from EarthSciData.jl's `era5.jl` (`ERA5PressureLevelFileSet`).
//!
//! It turns the loader-declared fields — variable list, pressure levels, spatial
//! domain bounds, and a month — into the **canonical CDS request** and the
//! `cds://` resolved URL the cache fetches through the [`cds`](crate::transport)
//! transport. One `cds://` URL per month, mirroring `era5.jl`'s
//! `era5_pl_<YYYY>_<MM>.nc` monthly files.
//!
//! Scope: the **request mapping** only. Reducing a `DomainInfo` to lon/lat bounds
//! and pressure-level indices (Proj transforms, grid sampling) stays upstream —
//! exactly as `era5.jl` reduces the domain *before* building the request. Here
//! the caller supplies the already-resolved bounds and level indices.
//!
//! Variable, pressure-level, day, and time lists are **sorted** before encoding,
//! so the same logical request always produces the same cache key regardless of
//! the order the caller passed them in.

use std::collections::BTreeMap;

use crate::error::{Error, Result};
use crate::transport::build_cds_url;

/// The CDS dataset identifier for ERA5 reanalysis on pressure levels.
pub const ERA5_PRESSURE_LEVELS_DATASET: &str = "reanalysis-era5-pressure-levels";

/// ERA5 pressure levels in hPa, surface (1000) → top (1), as `era5.jl`.
/// Level index 1 = 1000 hPa … index 37 = 1 hPa (1-based, matching the Julia port).
pub const ERA5_PRESSURE_LEVELS_HPA: [i32; 37] = [
    1000, 975, 950, 925, 900, 875, 850, 825, 800, 775, 750, //
    700, 650, 600, 550, 500, 450, 400, 350, 300, //
    250, 225, 200, 175, 150, 125, 100, //
    70, 50, 30, 20, 10, 7, 5, 3, 2, 1,
];

/// CDS long variable name → NetCDF short name, as `era5.jl`'s `ERA5_VARIABLES`.
/// Sorted by long name so [`all_variables`] is deterministic.
pub const ERA5_VARIABLES: &[(&str, &str)] = &[
    ("divergence", "d"),
    ("fraction_of_cloud_cover", "cc"),
    ("geopotential", "z"),
    ("ozone_mass_mixing_ratio", "o3"),
    ("potential_vorticity", "pv"),
    ("relative_humidity", "r"),
    ("specific_cloud_ice_water_content", "ciwc"),
    ("specific_cloud_liquid_water_content", "clwc"),
    ("specific_humidity", "q"),
    ("specific_rain_water_content", "crwc"),
    ("specific_snow_water_content", "cswc"),
    ("temperature", "t"),
    ("u_component_of_wind", "u"),
    ("v_component_of_wind", "v"),
    ("vertical_velocity", "w"),
    ("vorticity", "vo"),
];

/// All CDS long variable names, sorted (the `era5.jl` default request set).
pub fn all_variables() -> Vec<String> {
    ERA5_VARIABLES
        .iter()
        .map(|(long, _)| (*long).to_string())
        .collect()
}

/// The NetCDF short name for a CDS long variable name, if known.
pub fn variable_short_name(cds_name: &str) -> Option<&'static str> {
    ERA5_VARIABLES
        .iter()
        .find(|(long, _)| *long == cds_name)
        .map(|(_, short)| *short)
}

/// Map 1-based pressure-level indices to hPa values, as `era5.jl`'s
/// `ERA5_PRESSURE_LEVELS_HPA[round.(Int, levrange)]`. An index of 0 or one past
/// the table length is an error.
pub fn pressure_levels_from_indices(indices: &[usize]) -> Result<Vec<i32>> {
    indices
        .iter()
        .map(|&i| {
            if i == 0 || i > ERA5_PRESSURE_LEVELS_HPA.len() {
                Err(Error::Format {
                    format: "era5".to_string(),
                    detail: format!(
                        "pressure-level index {i} out of range 1..={}",
                        ERA5_PRESSURE_LEVELS_HPA.len()
                    ),
                })
            } else {
                Ok(ERA5_PRESSURE_LEVELS_HPA[i - 1])
            }
        })
        .collect()
}

/// CDS `area` = `[north, west, south, east]` (degrees) from lon/lat bounds, with
/// `era5.jl`'s ±1° padding and ceil/floor rounding:
/// `north = ⌈lat_max+1⌉`, `west = ⌊lon_min-1⌋`, `south = ⌊lat_min-1⌋`,
/// `east = ⌈lon_max+1⌉`. Each tuple is `(a, b)` in either order.
pub fn area_from_bounds(lon: (f64, f64), lat: (f64, f64)) -> [i32; 4] {
    let (lon_min, lon_max) = (lon.0.min(lon.1), lon.0.max(lon.1));
    let (lat_min, lat_max) = (lat.0.min(lat.1), lat.0.max(lat.1));
    [
        (lat_max + 1.0).ceil() as i32,  // north
        (lon_min - 1.0).floor() as i32, // west
        (lat_min - 1.0).floor() as i32, // south
        (lon_max + 1.0).ceil() as i32,  // east
    ]
}

/// Inclusive list of `(year, month)` pairs spanning `[start, end]` — the months
/// an ERA5 retrieval is decomposed into (one `cds://` file each). Empty when
/// `start > end`.
pub fn months_in_range(start: (i32, u32), end: (i32, u32)) -> Vec<(i32, u32)> {
    let mut out = Vec::new();
    let (mut y, mut m) = start;
    while (y, m) <= end {
        out.push((y, m));
        m += 1;
        if m > 12 {
            m = 1;
            y += 1;
        }
    }
    out
}

/// Days `1..=N` of a Gregorian `month`, leap-year aware. Errors for `month`
/// outside `1..=12`.
pub fn days_in_month(year: i32, month: u32) -> Result<Vec<u32>> {
    let n = match month {
        1 | 3 | 5 | 7 | 8 | 10 | 12 => 31,
        4 | 6 | 9 | 11 => 30,
        2 if is_leap_year(year) => 29,
        2 => 28,
        _ => {
            return Err(Error::Format {
                format: "era5".to_string(),
                detail: format!("invalid month {month} (expected 1..=12)"),
            })
        }
    };
    Ok((1..=n).collect())
}

/// Proleptic Gregorian leap-year test.
fn is_leap_year(year: i32) -> bool {
    (year % 4 == 0 && year % 100 != 0) || year % 400 == 0
}

/// An ERA5 pressure-level CDS retrieval, parameterized by the loader-declared
/// fields. Produces the canonical CDS request JSON and the `cds://` URL for a
/// given month.
#[derive(Debug, Clone)]
pub struct Era5PressureLevels {
    /// CDS long variable names to retrieve (see [`all_variables`]).
    pub variables: Vec<String>,
    /// Pressure levels in hPa (see [`pressure_levels_from_indices`]).
    pub pressure_levels: Vec<i32>,
    /// CDS `area` = `[north, west, south, east]` in degrees (see
    /// [`area_from_bounds`]).
    pub area: [i32; 4],
}

impl Era5PressureLevels {
    /// The CDS dataset this maps to.
    pub const DATASET: &'static str = ERA5_PRESSURE_LEVELS_DATASET;

    /// The canonical CDS request JSON for one month. `year` is a full year (e.g.
    /// 2018); `month` is `1..=12`; `days` are 1-based days-of-month. Variable,
    /// pressure-level, day, and time lists are sorted (pressure levels
    /// descending, as `era5.jl`) and de-duplicated so the encoding — hence the
    /// cache key — is independent of caller order.
    pub fn request_json(&self, year: i32, month: u32, days: &[u32]) -> String {
        let mut variable = self.variables.clone();
        variable.sort();
        variable.dedup();

        let mut plevels = self.pressure_levels.clone();
        plevels.sort_unstable_by(|a, b| b.cmp(a)); // descending, as era5.jl
        plevels.dedup();
        let pressure_level: Vec<String> = plevels.iter().map(|p| p.to_string()).collect();

        let mut day_nums = days.to_vec();
        day_nums.sort_unstable();
        day_nums.dedup();
        let day: Vec<String> = day_nums.iter().map(|d| format!("{d:02}")).collect();

        let time: Vec<String> = (0..24).map(|h| format!("{h:02}:00")).collect();

        // A BTreeMap serializes its keys in sorted order regardless of whether
        // serde_json's `preserve_order` feature is enabled elsewhere in the
        // dependency tree — so the canonical encoding is stable by construction.
        let mut req: BTreeMap<&str, serde_json::Value> = BTreeMap::new();
        req.insert("product_type", serde_json::json!(["reanalysis"]));
        req.insert("variable", serde_json::json!(variable));
        req.insert("pressure_level", serde_json::json!(pressure_level));
        req.insert("year", serde_json::json!([year.to_string()]));
        req.insert("month", serde_json::json!([format!("{month:02}")]));
        req.insert("day", serde_json::json!(day));
        req.insert("time", serde_json::json!(time));
        req.insert("data_format", serde_json::json!("netcdf"));
        req.insert("download_format", serde_json::json!("unarchived"));
        req.insert("area", serde_json::json!(self.area.to_vec()));

        serde_json::to_string(&req).expect("BTreeMap<&str, Value> serializes")
    }

    /// The `cds://` resolved URL for one month — the cache key source.
    pub fn cds_url(&self, year: i32, month: u32, days: &[u32]) -> String {
        build_cds_url(Self::DATASET, &self.request_json(year, month, days))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::transport::parse_cds_url;

    #[test]
    fn variable_table_is_sorted_and_complete() {
        let vars = all_variables();
        assert_eq!(vars.len(), 16);
        assert_eq!(vars.first().map(String::as_str), Some("divergence"));
        let mut sorted = vars.clone();
        sorted.sort();
        assert_eq!(vars, sorted, "all_variables() must be sorted");
        assert_eq!(variable_short_name("temperature"), Some("t"));
        assert_eq!(variable_short_name("vertical_velocity"), Some("w"));
        assert_eq!(variable_short_name("not_a_variable"), None);
    }

    #[test]
    fn pressure_level_index_mapping() {
        assert_eq!(
            pressure_levels_from_indices(&[1, 12, 37]).unwrap(),
            vec![1000, 700, 1]
        );
        assert!(pressure_levels_from_indices(&[0]).is_err());
        assert!(pressure_levels_from_indices(&[38]).is_err());
    }

    #[test]
    fn area_padding_and_rounding() {
        // lon (-100.4, -80.6), lat (30.2, 45.8):
        // N=⌈46.8⌉=47, W=⌊-101.4⌋=-102, S=⌊29.2⌋=29, E=⌈-79.6⌉=-79.
        assert_eq!(
            area_from_bounds((-100.4, -80.6), (30.2, 45.8)),
            [47, -102, 29, -79]
        );
        // Tuple order does not matter (extrema are taken internally).
        assert_eq!(
            area_from_bounds((-80.6, -100.4), (45.8, 30.2)),
            [47, -102, 29, -79]
        );
    }

    #[test]
    fn months_and_days() {
        assert_eq!(
            months_in_range((2018, 11), (2019, 2)),
            vec![(2018, 11), (2018, 12), (2019, 1), (2019, 2)]
        );
        assert_eq!(months_in_range((2019, 5), (2019, 5)), vec![(2019, 5)]);
        assert!(months_in_range((2019, 6), (2019, 5)).is_empty());

        assert_eq!(days_in_month(2020, 2).unwrap().len(), 29); // leap
        assert_eq!(days_in_month(2019, 2).unwrap().len(), 28);
        assert_eq!(days_in_month(2000, 2).unwrap().len(), 29); // /400
        assert_eq!(days_in_month(1900, 2).unwrap().len(), 28); // /100 not /400
        assert_eq!(days_in_month(2018, 4).unwrap().len(), 30);
        assert_eq!(
            days_in_month(2018, 1).unwrap(),
            (1..=31).collect::<Vec<_>>()
        );
        assert!(days_in_month(2018, 13).is_err());
    }

    #[test]
    fn request_json_is_canonical_and_faithful() {
        let era5 = Era5PressureLevels {
            // Deliberately unsorted to prove the encoder sorts.
            variables: vec!["temperature".into(), "geopotential".into()],
            pressure_levels: vec![850, 1000],
            area: [50, -130, 20, -60],
        };
        let json = era5.request_json(2018, 1, &[8, 1]);
        let v: serde_json::Value = serde_json::from_str(&json).unwrap();

        assert_eq!(v["product_type"], serde_json::json!(["reanalysis"]));
        // Variables sorted ascending.
        assert_eq!(
            v["variable"],
            serde_json::json!(["geopotential", "temperature"])
        );
        // Pressure levels sorted DESCENDING, stringified.
        assert_eq!(v["pressure_level"], serde_json::json!(["1000", "850"]));
        assert_eq!(v["year"], serde_json::json!(["2018"]));
        assert_eq!(v["month"], serde_json::json!(["01"]));
        // Days sorted ascending, zero-padded.
        assert_eq!(v["day"], serde_json::json!(["01", "08"]));
        assert_eq!(v["time"].as_array().unwrap().len(), 24);
        assert_eq!(v["time"][0], serde_json::json!("00:00"));
        assert_eq!(v["time"][23], serde_json::json!("23:00"));
        assert_eq!(v["data_format"], serde_json::json!("netcdf"));
        assert_eq!(v["download_format"], serde_json::json!("unarchived"));
        assert_eq!(v["area"], serde_json::json!([50, -130, 20, -60]));
    }

    #[test]
    fn request_json_independent_of_caller_order() {
        let a = Era5PressureLevels {
            variables: vec!["temperature".into(), "geopotential".into()],
            pressure_levels: vec![850, 1000],
            area: [50, -130, 20, -60],
        };
        let b = Era5PressureLevels {
            variables: vec!["geopotential".into(), "temperature".into()],
            pressure_levels: vec![1000, 850],
            area: [50, -130, 20, -60],
        };
        // Same logical request, different input order ⇒ identical encoding ⇒
        // identical cache key.
        assert_eq!(
            a.request_json(2018, 1, &[1, 8]),
            b.request_json(2018, 1, &[8, 1])
        );
    }

    #[test]
    fn cds_url_is_parseable() {
        let era5 = Era5PressureLevels {
            variables: vec!["temperature".into()],
            pressure_levels: vec![1000],
            area: [50, -130, 20, -60],
        };
        let url = era5.cds_url(2018, 11, &[8]);
        assert!(url.starts_with("cds://reanalysis-era5-pressure-levels?"));
        let (dataset, request_json) = parse_cds_url(&url).unwrap();
        assert_eq!(dataset, Era5PressureLevels::DATASET);
        assert_eq!(request_json, era5.request_json(2018, 11, &[8]));
    }
}
