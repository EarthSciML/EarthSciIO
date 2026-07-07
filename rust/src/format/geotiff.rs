//! Pure-Rust **GeoTIFF** reader behind the `format` registry — raster bands on a
//! native lon/lat (or x/y) grid. Decode parity with the Python
//! ([`earthsciio.readers.GeoTIFFReader`]) and Julia GeoTIFF readers: the same
//! [`NativeDataset`] for the USGS 3DEP / LANDFIRE rasters, so ESS sees one shape
//! across all three tracks (`esio-9nb`, data-providers plan §4.6).
//!
//! # Why the `tiff` crate, not GDAL/`geotiff`
//!
//! The crate deliberately avoids a C/FFI toolchain (see `Cargo.toml`: rustls is
//! chosen so the build needs "a C compiler alone, no clang/bindgen"), and the
//! `netcdf` reader is a focused pure-Rust decoder for the same reason. The
//! `tiff` crate is pure Rust and decodes exactly the rasters the loaders fetch —
//! a single-band F32 IEEEFP GeoTIFF (the no-auth USGS ImageServer `pixelType=F32`
//! export), tiled or stripped, uncompressed. GDAL (`rasterio`) and the higher
//! level `geotiff` crate (a thin, early-stage wrapper over this same `tiff`
//! crate, pulling in `geo-types`/`delaunator`/`geo-index`) buy georeferencing
//! machinery this reader gets from two tags directly — so neither earns its
//! weight here. The georef tags (`ModelPixelScaleTag` + `ModelTiepointTag`) are
//! parsed exactly as the Python `tifffile` fallback parses them.
//!
//! Compressed / colour-mapped / multi-sample-per-pixel-planar TIFFs are out of
//! scope on purpose: they land later as extensions to this same reader, a
//! [`crate::Provider`]-free change (the registry invariant).

use std::path::Path;

use tiff::decoder::{Decoder, DecodingResult};
use tiff::tags::Tag;

use crate::error::{Error, Result};

use super::{ArrayData, Coord, DType, NativeDataset, NativeField, Reader, Selection};

// GeoTIFF georef tags (OGC GeoTIFF 1.1 §C; the values `tifffile` reads too).
const MODEL_PIXEL_SCALE: u16 = 33550; // (sx, sy, sz) model units per pixel
const MODEL_TIEPOINT: u16 = 33922; // (i, j, k, x, y, z): raster point ↔ model point
const GEO_KEY_DIRECTORY: u16 = 34735; // GeoKeyDirectoryTag (GTModelTypeGeoKey = 1024)
const GT_MODEL_TYPE_KEY: u16 = 1024; // 1 = projected, 2 = geographic

/// The active `geotiff` reader: pure-Rust raster decode + tiepoint georef.
#[derive(Debug, Default, Clone, Copy)]
pub struct GeoTiffReader;

impl GeoTiffReader {
    /// Construct the reader.
    pub fn new() -> Self {
        Self
    }
}

impl Reader for GeoTiffReader {
    fn formats(&self) -> &'static [&'static str] {
        &["geotiff"]
    }

    fn extensions(&self) -> &'static [&'static str] {
        &["tif", "tiff"]
    }

    fn read_native(
        &self,
        blob_path: &Path,
        variables: &[String],
        _select: &Selection,
    ) -> Result<NativeDataset> {
        // Selection::All is the only variant today; the whole raster is read.
        let file =
            std::fs::File::open(blob_path).map_err(|e| Error::io(Some(blob_path.to_path_buf()), e))?;
        decode(file, variables)
    }
}

/// Decode a GeoTIFF blob into raster-band native fields + a native lon/lat grid.
fn decode<R: std::io::Read + std::io::Seek>(
    reader: R,
    variables: &[String],
) -> Result<NativeDataset> {
    let mut dec = Decoder::new(reader).map_err(tiff_err)?;
    let (width, height) = dec.dimensions().map_err(tiff_err)?;
    let (w, h) = (width as usize, height as usize);
    let ncell = w.checked_mul(h).ok_or_else(|| fmt_err("raster dimensions overflow"))?;
    if ncell == 0 {
        return Err(fmt_err("empty raster (zero width or height)"));
    }

    // Whole image, decoded row-major (row 0 = top). Any numeric sample type is
    // widened to f64 (the CF/native contract is float64); the fetched rasters are
    // F32 IEEEFP but a reader that also accepts int/other-float DEMs is free.
    let flat = to_f64(dec.read_image().map_err(tiff_err)?)?;
    if flat.len() % ncell != 0 {
        return Err(fmt_err(&format!(
            "decoded sample count {} is not a multiple of {w}x{h}",
            flat.len()
        )));
    }
    let nbands = flat.len() / ncell;
    if nbands == 0 {
        return Err(fmt_err("raster has zero bands"));
    }

    // Band names: GDAL's 1-based `Band1..BandN` convention (the Python reader's
    // default, and the name the LANDFIRE loader's `file_variable: "Band1"`
    // matches). `variables`, when non-empty, filters to those band names — the
    // same "restrict to on-disk names" semantics as the netcdf reader.
    let names: Vec<String> = (1..=nbands).map(|i| format!("Band{i}")).collect();
    let want: Option<std::collections::HashSet<&str>> = if variables.is_empty() {
        None
    } else {
        Some(variables.iter().map(String::as_str).collect())
    };
    if let Some(w) = &want {
        for v in w {
            if !names.iter().any(|n| n.as_str() == *v) {
                return Err(fmt_err(&format!(
                    "requested band {v:?} not in GeoTIFF (present: {names:?})"
                )));
            }
        }
    }

    // Georef: derive north-up cell-center axes from ModelPixelScale + Tiepoint
    // (y-up model space; raster rows increase downward). Geographic vs projected
    // decides the axis names (lat/lon vs y/x), from GTModelTypeGeoKey.
    let scale = dec.get_tag_f64_vec(Tag::Unknown(MODEL_PIXEL_SCALE)).ok();
    let tie = dec.get_tag_f64_vec(Tag::Unknown(MODEL_TIEPOINT)).ok();
    let geographic = geokey_value(&mut dec, GT_MODEL_TYPE_KEY) != Some(1);
    let (ydim, xdim) = if geographic { ("lat", "lon") } else { ("y", "x") };

    let axes = match (scale.as_deref(), tie.as_deref()) {
        (Some(s), Some(t)) if s.len() >= 2 && t.len() >= 6 => {
            let (sx, sy) = (s[0], s[1]);
            let (i0, j0) = (t[0], t[1]);
            let (x0, y0) = (t[3], t[4]);
            let xs: Vec<f64> = (0..w).map(|i| x0 + (i as f64 - i0 + 0.5) * sx).collect();
            let ys: Vec<f64> = (0..h).map(|j| y0 - (j as f64 - j0 + 0.5) * sy).collect();
            Some((xs, ys))
        }
        // No tiepoint georef (a plain TIFF) — return the raster with no coords.
        // The georef is optional for a loader whose consumer supplies the grid.
        _ => None,
    };

    let mut out = NativeDataset::default();
    for (b, name) in names.iter().enumerate() {
        if let Some(w) = &want {
            if !w.contains(name.as_str()) {
                continue;
            }
        }
        // De-interleave contiguous (pixel-major) samples: band b is every
        // nbands-th value. Single-band rasters (the fetched DEMs) fall through
        // with stride 1.
        let data: Vec<f64> = if nbands == 1 {
            flat.clone()
        } else {
            flat.iter().skip(b).step_by(nbands).copied().collect()
        };
        out.variables.insert(
            name.clone(),
            NativeField {
                dtype: DType::Float64,
                dims: vec![ydim.to_string(), xdim.to_string()],
                shape: vec![h, w],
                data: ArrayData::F64(data),
                fill_value: None,
            },
        );
    }

    if let Some((xs, ys)) = axes {
        out.coords.insert(xdim.to_string(), axis_coord(xdim, xs));
        out.coords.insert(ydim.to_string(), axis_coord(ydim, ys));
    }
    Ok(out)
}

/// A 1-D coordinate native field (no CF time metadata — spatial axis).
fn axis_coord(dim: &str, values: Vec<f64>) -> Coord {
    Coord {
        field: NativeField {
            dtype: DType::Float64,
            dims: vec![dim.to_string()],
            shape: vec![values.len()],
            data: ArrayData::F64(values),
            fill_value: None,
        },
        units: None,
        calendar: None,
    }
}

/// Widen a decoded TIFF image buffer of any numeric sample type to `f64`.
fn to_f64(res: DecodingResult) -> Result<Vec<f64>> {
    let v = match res {
        DecodingResult::F32(v) => v.into_iter().map(|x| x as f64).collect(),
        DecodingResult::F64(v) => v,
        DecodingResult::U8(v) => v.into_iter().map(|x| x as f64).collect(),
        DecodingResult::U16(v) => v.into_iter().map(|x| x as f64).collect(),
        DecodingResult::U32(v) => v.into_iter().map(|x| x as f64).collect(),
        DecodingResult::U64(v) => v.into_iter().map(|x| x as f64).collect(),
        DecodingResult::I8(v) => v.into_iter().map(|x| x as f64).collect(),
        DecodingResult::I16(v) => v.into_iter().map(|x| x as f64).collect(),
        DecodingResult::I32(v) => v.into_iter().map(|x| x as f64).collect(),
        DecodingResult::I64(v) => v.into_iter().map(|x| x as f64).collect(),
        // f16 (and any future sample type) — the fetched DEMs are F32/F64, so a
        // half-float raster is out of scope for this focused reader.
        _ => {
            return Err(fmt_err(
                "unsupported GeoTIFF sample type (f16 or other; expected F32/F64/int)",
            ))
        }
    };
    Ok(v)
}

/// Look up a scalar GeoKey value (TIFFTagLocation == 0, inline) from the
/// GeoKeyDirectoryTag. Returns `None` if the tag or key is absent. Layout: a
/// `u16` header `[version, revA, revB, numKeys]` then `numKeys` 4-tuples
/// `[keyId, tagLocation, count, valueOrOffset]` (OGC GeoTIFF 1.1 §B).
fn geokey_value<R: std::io::Read + std::io::Seek>(dec: &mut Decoder<R>, key_id: u16) -> Option<u16> {
    let dir = dec.get_tag_u16_vec(Tag::Unknown(GEO_KEY_DIRECTORY)).ok()?;
    if dir.len() < 4 {
        return None;
    }
    let n = dir[3] as usize;
    for k in 0..n {
        let base = 4 + k * 4;
        if base + 3 >= dir.len() {
            break;
        }
        if dir[base] == key_id && dir[base + 1] == 0 {
            return Some(dir[base + 3]);
        }
    }
    None
}

fn tiff_err(e: tiff::TiffError) -> Error {
    Error::Format {
        format: "geotiff".to_string(),
        detail: e.to_string(),
    }
}

fn fmt_err(detail: &str) -> Error {
    Error::Format {
        format: "geotiff".to_string(),
        detail: detail.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;
    use tiff::encoder::{colortype, TiffEncoder};

    /// Encode a `width`×`height` single-band F32 GeoTIFF (row-major, row 0 = top)
    /// with ModelPixelScale + ModelTiepoint + a geographic GeoKeyDirectory, into
    /// an in-memory buffer — the shape of the ArcGIS ImageServer `pixelType=F32`
    /// exports the DEM loaders fetch.
    fn encode_geographic(
        width: u32,
        height: u32,
        data: &[f32],
        scale: [f64; 3],
        tie: [f64; 6],
    ) -> Vec<u8> {
        let mut buf = Cursor::new(Vec::new());
        {
            let mut enc = TiffEncoder::new(&mut buf).unwrap();
            let mut img = enc
                .new_image::<colortype::Gray32Float>(width, height)
                .unwrap();
            img.encoder()
                .write_tag(Tag::Unknown(MODEL_PIXEL_SCALE), &scale[..])
                .unwrap();
            img.encoder()
                .write_tag(Tag::Unknown(MODEL_TIEPOINT), &tie[..])
                .unwrap();
            // GeoKeyDirectory: header [1,1,0,1] + one key (GTModelType=2=geographic).
            img.encoder()
                .write_tag(
                    Tag::Unknown(GEO_KEY_DIRECTORY),
                    &[1u16, 1, 0, 1, GT_MODEL_TYPE_KEY, 0, 1, 2][..],
                )
                .unwrap();
            img.write_data(data).unwrap();
        }
        buf.into_inner()
    }

    #[test]
    fn decodes_band_grid_and_geographic_axes() {
        // 3 lon × 2 lat, row 0 = north. Row-major values.
        let data = [10.0f32, 20.0, 30.0, 40.0, 50.0, 60.0];
        let bytes = encode_geographic(3, 2, &data, [0.1, 0.2, 0.0], [0.0, 0.0, 0.0, 100.0, 50.0, 0.0]);
        let ds = decode(Cursor::new(bytes), &[]).expect("decode");

        let band = ds.variables.get("Band1").expect("Band1");
        assert_eq!(band.dims, vec!["lat".to_string(), "lon".to_string()]);
        assert_eq!(band.shape, vec![2, 3]);
        let ArrayData::F64(v) = &band.data else { panic!("f64 band") };
        assert_eq!(v, &[10.0, 20.0, 30.0, 40.0, 50.0, 60.0]);

        // Cell-center axes from the tiepoint: lon east-increasing, lat north-down.
        let lon = &ds.coords.get("lon").expect("lon").field.data;
        let lat = &ds.coords.get("lat").expect("lat").field.data;
        let (ArrayData::F64(lon), ArrayData::F64(lat)) = (lon, lat) else {
            panic!("f64 coords")
        };
        assert!((lon[0] - 100.05).abs() < 1e-9 && (lon[2] - 100.25).abs() < 1e-9);
        assert!((lat[0] - 49.9).abs() < 1e-9 && (lat[1] - 49.7).abs() < 1e-9);
    }

    #[test]
    fn band_names_filter_and_default() {
        let data = [1.0f32, 2.0, 3.0, 4.0];
        let bytes = encode_geographic(2, 2, &data, [1.0, 1.0, 0.0], [0.0, 0.0, 0.0, 0.0, 0.0, 0.0]);
        // Filtering to the default single band name keeps it.
        let ds = decode(Cursor::new(bytes.clone()), &["Band1".to_string()]).expect("decode");
        assert!(ds.variables.contains_key("Band1"));
        // A requested-but-absent band is an error, not a silent drop.
        let err = decode(Cursor::new(bytes), &["elevation".to_string()]).unwrap_err();
        assert!(err.to_string().contains("Band"));
    }
}
