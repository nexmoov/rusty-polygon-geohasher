use geo::{
    algorithm::{centroid::Centroid, contains::Contains},
    BoundingRect, Intersects, Polygon,
};
use geo_types::Geometry as GtGeometry;
use geohash::{decode_bbox, encode, neighbors, GeohashError};
use py_geo_interface::Geometry;
use pyo3::types::PyAny;
use pyo3::wrap_pyfunction;
use pyo3::{exceptions::PyValueError, prelude::*};
use std::collections::{HashSet, VecDeque};

fn polygons_to_geohashes(
    polygons: Vec<Polygon>,
    precision: usize,
    inner: bool,
) -> Result<HashSet<String>, GeohashError> {
    let mut inner_geohashes = HashSet::new();
    let mut outer_geohashes = HashSet::new();

    for polygon in &polygons {
        let envelope = polygon.bounding_rect().unwrap();

        let centroid = polygon.centroid().unwrap();
        let centroid_geohash = encode((centroid.x(), centroid.y()).into(), precision)?;

        let mut testing_geohashes = VecDeque::new();
        testing_geohashes.push_back(centroid_geohash);

        while let Some(current_geohash) = testing_geohashes.pop_front() {
            if inner_geohashes.contains(&current_geohash)
                || outer_geohashes.contains(&current_geohash)
            {
                continue;
            }

            let rect_bbox = decode_bbox(&current_geohash)?;
            let current_geohash_polygon = rect_bbox.to_polygon();

            let condition = if inner {
                envelope.contains(&rect_bbox)
            } else {
                envelope.intersects(&rect_bbox)
            };
            if !condition {
                continue;
            }

            if inner {
                if polygon.contains(&current_geohash_polygon) {
                    inner_geohashes.insert(current_geohash.clone());
                } else {
                    outer_geohashes.insert(current_geohash.clone());
                }
            } else {
                if polygon.intersects(&current_geohash_polygon) {
                    inner_geohashes.insert(current_geohash.clone());
                } else {
                    outer_geohashes.insert(current_geohash.clone());
                }
            }

            if let Ok(rez) = neighbors(&current_geohash) {
                for neighbor in [rez.sw, rez.s, rez.se, rez.w, rez.e, rez.nw, rez.n, rez.ne] {
                    if !inner_geohashes.contains(&neighbor) && !outer_geohashes.contains(&neighbor)
                    {
                        testing_geohashes.push_back(neighbor.to_string());
                    }
                }
            }
        }
    }
    Ok(inner_geohashes)
}

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

    polygons_to_geohashes(polygons, precision, inner)
        .map_err(|e| pyo3::exceptions::PyValueError::new_err(format!("{:?}", e)))
}

#[pymodule]
fn geohash_polygon(m: &Bound<'_, PyModule>) -> PyResult<()> {
    m.add_function(wrap_pyfunction!(polygon_to_geohashes, m)?)?;
    Ok(())
}
