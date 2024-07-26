#![recursion_limit = "512"]
use geo::{
    algorithm::{centroid::Centroid, contains::Contains},
    BoundingRect, Coord, Intersects, LineString, Polygon,
};
use geohash::{decode, encode, neighbors};
use pyo3::prelude::*;
use pyo3::types::{PyAny, PyTuple};
use pyo3::wrap_pyfunction;
use std::collections::{HashSet, VecDeque};

fn get_length(obj: &PyAny) -> PyResult<usize> {
    // Ensure the object has a length
    obj.len().map_err(|e| e.into())
}

#[pyfunction]
fn object_length(#[pyo3(from_py_with = "get_length")] length: usize) -> usize {
    length
}

#[pyfunction]
fn polygon_to_geohashes(
    _py: Python,
    py_polygon: &PyAny,
    precision: usize,
    inner: bool,
) -> PyResult<HashSet<String>> {
    let mut inner_geohashes = HashSet::new();
    let mut outer_geohashes = HashSet::new();

    let geoms_collection: &PyAny = py_polygon.getattr("geoms")?.extract()?;

    for curr_geom_idx in 0..=get_length(geoms_collection)? - 1 {
        let curr_geom: &PyAny = geoms_collection.get_item(curr_geom_idx)?;
        let coords_list: &PyAny = curr_geom
            .getattr("exterior")?
            .getattr("coords")?
            .extract()?;
        let mut coordinates = Vec::<Coord<f64>>::new();
        for coords_idx in 0..=get_length(coords_list)? - 1 {
            let item = coords_list.get_item(coords_idx)?;
            let tuple: &PyTuple = item.extract()?;
            let y: f64 = tuple.get_item(0).expect("REASON").extract()?;
            let x: f64 = tuple.get_item(1).expect("REASON").extract()?;
            coordinates.push(Coord::<f64> { x, y });
        }

        let polygon = Polygon::new(LineString::from(coordinates), vec![]);


        let envelope = polygon.bounding_rect().unwrap();
        let poly_envelope = envelope.to_polygon();

        let centroid = polygon.centroid().unwrap();
        let centroid_geohash = encode((centroid.y(), centroid.x()).into(), precision)
            .map_err(|e| pyo3::exceptions::PyValueError::new_err(format!("{:?}", e)))?;

        let mut testing_geohashes = VecDeque::new();
        testing_geohashes.push_back(centroid_geohash);

        while let Some(current_geohash) = testing_geohashes.pop_front() {
            if !inner_geohashes.contains(&current_geohash)
                && !outer_geohashes.contains(&current_geohash)
            {
                let (decoded_centroid, lat_offset, lng_offset) = decode(&current_geohash)
                    .map_err(|e| pyo3::exceptions::PyValueError::new_err(format!("{:?}", e)))?;

                let corner_1 = Coord::<f64> {
                    y: decoded_centroid.x - lat_offset,
                    x: decoded_centroid.y - lng_offset,
                };
                let corner_2 = Coord::<f64> {
                    y: decoded_centroid.x + lat_offset,
                    x: decoded_centroid.y - lng_offset,
                };
                let corner_3 = Coord::<f64> {
                    y: decoded_centroid.x + lat_offset,
                    x: decoded_centroid.y + lng_offset,
                };
                let corner_4 = Coord::<f64> {
                    y: decoded_centroid.x - lat_offset,
                    x: decoded_centroid.y + lng_offset,
                };
                let current_polygon = Polygon::new(
                    vec![corner_1, corner_2, corner_3, corner_4, corner_1].into(),
                    vec![],
                );

                let condition = if inner {
                    poly_envelope.contains(&current_polygon)
                } else {
                    poly_envelope.intersects(&current_polygon)
                };
                if condition {
                    if inner {
                        if polygon.contains(&current_polygon) {
                            inner_geohashes.insert(current_geohash.clone());
                        } else {
                            outer_geohashes.insert(current_geohash.clone());
                        }
                    } else {
                        if polygon.intersects(&current_polygon) {
                            inner_geohashes.insert(current_geohash.clone());
                        } else {
                            outer_geohashes.insert(current_geohash.clone());
                        }
                    }

                    if let Ok(rez) = neighbors(&current_geohash) {
                        for neighbor in
                            vec![rez.sw, rez.s, rez.se, rez.w, rez.e, rez.nw, rez.n, rez.ne]
                        {
                            if !inner_geohashes.contains(&neighbor)
                                && !outer_geohashes.contains(&neighbor)
                            {
                                testing_geohashes.push_back(neighbor.to_string());
                            }
                        }
                    }
                }
            }
        }
    }
    Ok(inner_geohashes)
}

#[pymodule]
fn geohash_polygon(_py: Python, m: &PyModule) -> PyResult<()> {
    m.add_function(wrap_pyfunction!(polygon_to_geohashes, m)?)?;
    Ok(())
}
