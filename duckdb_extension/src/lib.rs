extern crate duckdb;
extern crate duckdb_loadable_macros;
extern crate libduckdb_sys;
use std::panic::{catch_unwind, AssertUnwindSafe};
use std::io;
use duckdb::{
    core::{DataChunkHandle, Inserter, LogicalTypeHandle, LogicalTypeId},
    Connection, Result,
};
use duckdb::{
    core::{FlatVector},
    vscalar::{ScalarFunctionSignature, VScalar},
    vtab::arrow::WritableVector,
    types::DuckString,
};
use libduckdb_sys::{duckdb_blob, duckdb_string_t};
use duckdb_loadable_macros::duckdb_entrypoint_c_api;
use libduckdb_sys as ffi;
use std::error::Error;

use geo_types::{Geometry, Polygon};

use geozero::wkb::Wkb;
use geozero::wkt::Wkt;
use geozero::ToGeo;




// ---------- Scalar UDF returning LIST<VARCHAR> ----------

fn parse_input_to_polygons(
    ty: &LogicalTypeId,
    wkt_vals: Option<&[duckdb_string_t]>,
    blob_vals: Option<&[duckdb_blob]>, // type depends on your duckdb-rs version
    row: usize,
) -> Result<Vec<Polygon<f64>>, Box<dyn std::error::Error>> {
    let geom: Geometry<f64> = match *ty {
        LogicalTypeId::Varchar => {
            let mut raw = wkt_vals.unwrap()[row];
            let s = DuckString::new(&mut raw).as_str();
            Wkt(s.to_string()).to_geo()?
        }
        LogicalTypeId::Blob => {
            let b = blob_vals.unwrap()[row];
            let bytes = unsafe { std::slice::from_raw_parts(b.data as *const u8, b.size as usize) };
            Wkb(bytes.to_vec()).to_geo()?
        }
        _ => return Err("Unsupported input type (expected VARCHAR or BLOB)".into()),
    };

        let polys = match geom {
        Geometry::Polygon(p) => vec![p],
        Geometry::MultiPolygon(mp) => mp.0,
        other => return Err(format!("Expected POLYGON or MULTIPOLYGON, got {other:?}").into()),
    };

    Ok(polys)
}

struct GeohashScalar;

impl VScalar for GeohashScalar {
    type State = ();

    unsafe fn invoke(
        _: &Self::State,
        input: &mut DataChunkHandle,
        output: &mut dyn WritableVector,
    ) -> duckdb::Result<(), Box<dyn std::error::Error>> {
        match catch_unwind(AssertUnwindSafe(|| {
            // ---- your existing logic goes here; return Result<(), Box<dyn Error>> ----
            GeohashScalar::inner_invoke(input, output)
        })) {
            Ok(r) => r,                                // propagate normal errors to DuckDB
            Err(_) => Err("rusty_quack: internal panic".into()),  // convert panic → Err
        }
    }


    fn signatures() -> Vec<ScalarFunctionSignature> {
        let child = LogicalTypeHandle::from(LogicalTypeId::Varchar);
        vec![
            // VARCHAR -> LIST<VARCHAR>
            ScalarFunctionSignature::exact(
                vec![
                    LogicalTypeHandle::from(LogicalTypeId::Varchar),
                    LogicalTypeHandle::from(LogicalTypeId::Integer),
                    LogicalTypeHandle::from(LogicalTypeId::Boolean),
                ],
                LogicalTypeHandle::list(&child),
            ),
            // BLOB (GEOMETRY) -> LIST<VARCHAR>
            ScalarFunctionSignature::exact(
                vec![
                    LogicalTypeHandle::from(LogicalTypeId::Blob),
                    LogicalTypeHandle::from(LogicalTypeId::Integer),
                    LogicalTypeHandle::from(LogicalTypeId::Boolean),
                ],
                LogicalTypeHandle::list(&child),
            ),
        ]
    }

}

impl GeohashScalar {

fn inner_invoke(
    input: &mut DataChunkHandle,
    output: &mut dyn WritableVector,
) -> duckdb::Result<(), Box<dyn std::error::Error>> {
    let n = input.len();

    // figure out if arg0 is VARCHAR or BLOB (GEOMETRY/WKB)
    let col0_vec: FlatVector = input.flat_vector(0);
    let arg0_type_id: LogicalTypeId = col0_vec.logical_type().id();

    let (wkt_vals_opt, blob_vals_opt): (Option<&[duckdb_string_t]>, Option<&[duckdb_blob]>) =
        match arg0_type_id {
            LogicalTypeId::Varchar => {
                let wkt_vals = col0_vec.as_slice_with_len::<duckdb_string_t>(n);
                (Some(wkt_vals), None)
            }
            LogicalTypeId::Blob => {
                let blob_vals = col0_vec.as_slice_with_len::<duckdb_blob>(n);
                (None, Some(blob_vals))
            }
            other => {
                return Err(format!(
                    "Unsupported type for argument 0: {:?} (expected VARCHAR or BLOB/GEOMETRY)",
                    other
                ).into());
            }
        };

    // ---- Read precision column defensively (col 1) ----
    let prec_vec: FlatVector = input.flat_vector(1);
    let prec_ty = prec_vec.logical_type().id();

    // Helper: read one row's precision as i64 (handles different integer widths + NULL)
    let get_prec = |row: usize| -> usize {
        let p_i64 = match prec_ty {
            LogicalTypeId::Tinyint  => prec_vec.as_slice_with_len::<i8>(n)[row]  as i64,
            LogicalTypeId::Smallint => prec_vec.as_slice_with_len::<i16>(n)[row] as i64,
            LogicalTypeId::Integer  => prec_vec.as_slice_with_len::<i32>(n)[row] as i64,
            LogicalTypeId::Bigint   => prec_vec.as_slice_with_len::<i64>(n)[row],
            _ => 6, // safe default if caller didn't sanitize
        };
        p_i64.clamp(1, 12) as usize
    };


    // col2: fully_contained_only (BOOLEAN)
    let full_col: FlatVector = input.flat_vector(2);
    let fully_only = full_col.as_slice_with_len::<bool>(n);

// ---- PASS 1: compute per-row results and total size ----
    let mut per_row: Vec<Vec<String>> = Vec::with_capacity(n);
    let mut total = 0usize;

    for row in 0..n {
        let polys = parse_input_to_polygons(&arg0_type_id, wkt_vals_opt, blob_vals_opt, row)?;
        let prec = get_prec(row);
        let gh = geohash_polygon::polygons_to_geohashes(polys, prec, fully_only[row])?;

        // sanitize (no NULs) and store
        let mut v = Vec::with_capacity(gh.len());
        for s in gh {
            if s.as_bytes().contains(&0) {
                return Err(format!("Row {row}: output contains NUL").into());
            }
            v.push(s);
        }
        total += v.len();
        per_row.push(v);
    }

    // ---- allocate exactly enough and write once ----
    let mut list = output.list_vector();
    let child = list.child(total);
    let mut off = 0usize;

    for (row, hashes) in per_row.iter().enumerate() {
        let start = off;
        for s in hashes {
            child.insert(off, s.as_str());
            off += 1;
        }
        list.set_entry(row, start, off - start);
    }
    list.set_len(n);
    Ok(())
}
}

// In your entrypoint, register the SCALAR (not table) function:
#[duckdb_entrypoint_c_api()]
pub unsafe fn extension_entrypoint(con: duckdb::Connection) -> Result<(), Box<dyn Error>> {
    con.register_scalar_function::<GeohashScalar>("rusty_quack")?; // call: SELECT rusty_quack(wkt, 6, FALSE)
    Ok(())
}
