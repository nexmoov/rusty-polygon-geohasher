use geo::{
    algorithm::{centroid::Centroid, contains::Contains},
    BoundingRect, Intersects, Polygon,
};
use geo_types::Geometry as GtGeometry;
use geohash::{decode_bbox, encode, neighbors};
use py_geo_interface::Geometry;
use pyo3::prelude::*;
use pyo3::types::PyAny;
use pyo3::wrap_pyfunction;
use std::collections::{HashSet, VecDeque};

#[pyfunction]
fn polygon_to_geohashes(
    _py: Python,
    py_polygon: Bound<'_, PyAny>,
    precision: usize,
    inner: bool,
) -> PyResult<HashSet<String>> {
    let mut polygons = Vec::<Polygon<f64>>::new();

    let geom: Geometry = match py_polygon.extract::<Geometry>() {
        Ok(geometry) => geometry,
        Err(e) => {
            return Err(pyo3::exceptions::PyValueError::new_err(
                format!("Exception while trying to extract Geometry. This function requires a Shapely Polygon or MultiPolygon. ({:?})", e
            )))
        }
    };

    if let Err(e) = {
        match geom.0 {
            GtGeometry::Polygon(polygon) => {
                polygons.push(polygon);
                Ok(())
            }
            GtGeometry::MultiPolygon(multipolygon) => {
                for polygon in multipolygon {
                    polygons.push(polygon);
                }
                Ok(())
            }
            _ => Err("The geometry is not a Polygon or MultiPolygon"),
        }
    } {
        return Err(pyo3::exceptions::PyValueError::new_err(e));
    }

    // geohashes that will be returned as inside the polygon
    let mut inner_geohashes = HashSet::new();
    // geohashes that were looked at and are outside the polygon
    let mut outer_geohashes = HashSet::new();
    // geohashes that are candidates to be tested.
    let mut candidate_geohashes = HashSet::new();

    //let mut tested = 0;
    for polygon in &polygons {
        let centroid = polygon.centroid().unwrap();
        let centroid_geohash = encode((centroid.x(), centroid.y()).into(), precision)
            .map_err(|e| pyo3::exceptions::PyValueError::new_err(format!("{:?}", e)))?;

        let mut testing_geohashes = VecDeque::new();
        testing_geohashes.push_back(centroid_geohash);

        while let Some(current_geohash) = testing_geohashes.pop_front() {
            //tested += 1;
            let rect_bbox = decode_bbox(&current_geohash)
                .map_err(|e| pyo3::exceptions::PyValueError::new_err(format!("{:?}", e)))?;
            let current_polygon = rect_bbox.to_polygon();

            if polygon.contains(&current_polygon)
                || (!inner && polygon.intersects(&current_polygon))
            {
                inner_geohashes.insert(current_geohash.clone());

                if let Ok(rez) = neighbors(&current_geohash) {
                    for neighbor in [rez.s, rez.w, rez.e, rez.n] {
                        if (!inner_geohashes.contains(&neighbor)
                            && !outer_geohashes.contains(&neighbor))
                            && !candidate_geohashes.contains(&neighbor)
                        {
                            testing_geohashes.push_back(neighbor.clone());
                            candidate_geohashes.insert(neighbor);
                        }
                    }
                }
            } else {
                outer_geohashes.insert(current_geohash);
            }
        }
    }
    //println!("tested: {}", tested);
    Ok(inner_geohashes)
}

#[pymodule]
fn geohash_polygon(m: &Bound<'_, PyModule>) -> PyResult<()> {
    m.add_function(wrap_pyfunction!(polygon_to_geohashes, m)?)?;
    Ok(())
}
