use geo::{
    algorithm::centroid::Centroid, Area, BoundingRect, Contains, Intersects, Point, Polygon, Rect,
};
use geohash::{decode_bbox, encode, neighbors, GeohashError};
use std::collections::{HashSet, VecDeque};

#[cfg(feature = "python")]
use {
    geo_types::Geometry as GtGeometry,
    py_geo_interface::Geometry,
    pyo3::{prelude::*, types::PyAny, wrap_pyfunction},
};

pub fn polygons_to_geohashes<PI>(
    polygons: PI,
    precision: usize,
    fully_contained_only: bool,
) -> Result<HashSet<String>, GeohashError>
where
    PI: IntoIterator<Item = Polygon>,
{
    let mut accepted_geohashes = HashSet::new();
    let mut rejected_geohashes = HashSet::new();

    for polygon in polygons {
        let polygon_exterior = polygon.exterior();
        let has_holes = !polygon.interiors().is_empty();

        // choose a seed inside the polygon
        let seed_point = seed_interior_point_fast(&polygon).unwrap_or_else(|| {
            // fallback if no interior found: use bbox center (rare)
            let b = polygon.bounding_rect().unwrap();
            Point::new((b.min().x + b.max().x) * 0.5, (b.min().y + b.max().y) * 0.5)
        });

        // convert to geohash and start BFS
        let mut testing_geohashes: VecDeque<String> = VecDeque::new();
        let seed_gh: String = encode((seed_point.x(), seed_point.y()).into(), precision)?;
        testing_geohashes.push_back(seed_gh);

        while let Some(current_geohash) = testing_geohashes.pop_front() {
            if accepted_geohashes.contains(&current_geohash)
                || rejected_geohashes.contains(&current_geohash)
            {
                continue;
            }

            let gh_bbox = decode_bbox(&current_geohash)?;
            let current_geohash_polygon = gh_bbox.to_polygon();

            // prune non-intersecting cells early and don't expand from them
            if !polygon.intersects(&current_geohash_polygon) {
                rejected_geohashes.insert(current_geohash.clone());
                continue;
            }

            let accept = if fully_contained_only {
                if has_holes {
                    // robust path when holes exist
                    polygon.contains(&current_geohash_polygon)
                } else {
                    // fast path for hole-free polygons (strict containment)
                    !polygon_exterior.intersects(current_geohash_polygon.exterior())
                        && current_geohash_polygon.unsigned_area() <= polygon.unsigned_area()
                }
            } else {
                // intersecting is enough
                true
            };

            if accept {
                accepted_geohashes.insert(current_geohash.clone());
            } else {
                rejected_geohashes.insert(current_geohash.clone());
            }

            if let Ok(rez) = neighbors(&current_geohash) {
                for neighbor in [rez.sw, rez.s, rez.se, rez.w, rez.e, rez.nw, rez.n, rez.ne] {
                    if !accepted_geohashes.contains(&neighbor)
                        && !rejected_geohashes.contains(&neighbor)
                    {
                        testing_geohashes.push_back(neighbor);
                    }
                }
            }
        }
    }
    Ok(accepted_geohashes)
}

pub fn polygons_to_geohashes_handbrake<PI>(
    polygons: PI,
    precision: usize,
    inner: bool,
) -> Result<HashSet<String>, GeohashError>
where
    PI: IntoIterator<Item = Polygon>,
{
    let mut inner_geohashes = HashSet::new();
    let mut outer_geohashes = HashSet::new();

    for polygon in polygons {
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

#[cfg(feature = "python")]
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

#[cfg(feature = "python")]
#[pymodule]
fn geohash_polygon(m: &Bound<'_, PyModule>) -> PyResult<()> {
    m.add_function(wrap_pyfunction!(polygon_to_geohashes, m)?)?;
    Ok(())
}

/// Ultra-fast interior seed with no RNG, no runtime trig.
/// Fixed set of offsets → tight upper bound on `contains` calls.
pub fn seed_interior_point_fast(poly: &Polygon) -> Option<Point> {
    let bbox: Rect = poly.bounding_rect()?;
    let (minx, miny, maxx, maxy) = (bbox.min().x, bbox.min().y, bbox.max().x, bbox.max().y);
    let bx = (maxx - minx).abs();
    let by = (maxy - miny).abs();
    let span = bx.max(by);

    // 1) centroid
    if let Some(c) = poly.centroid() {
        if poly.contains(&c) {
            return Some(c);
        }

        // 2) deterministic offsets around centroid (approximate unit circle, no trig)
        // 12 directions × 2 radii = 24 probes. Change radii for stricter/looser search.
        // Offsets are normalized-ish; we scale by bbox span to move off boundary.
        const OFFS: &[(f64, f64)] = &[
            // 12-direction star (clockwise), integer-friendly
            (1.0, 0.0),
            (0.866, 0.5),
            (0.5, 0.866),
            (0.0, 1.0),
            (-0.5, 0.866),
            (-0.866, 0.5),
            (-1.0, 0.0),
            (-0.866, -0.5),
            (-0.5, -0.866),
            (0.0, -1.0),
            (0.5, -0.866),
            (0.866, -0.5),
        ];
        // Very small step first to clear boundary noise; then a modest step
        let r1 = (span * 1e-6).max(1e-9);
        let r2 = span * 1e-4;

        // Elliptical scaling helps thin polygons aligned to axes
        let sx = if span > 0.0 { bx / span } else { 1.0 };
        let sy = if span > 0.0 { by / span } else { 1.0 };

        // Try r1 then r2
        for &r in &[r1, r2] {
            for &(dx, dy) in OFFS {
                let p = Point::new(c.x() + dx * r * sx, c.y() + dy * r * sy);
                if poly.contains(&p) {
                    return Some(p);
                }
            }
        }
    }

    // 3) bbox center
    let center = Point::new((minx + maxx) * 0.5, (miny + maxy) * 0.5);
    if poly.contains(&center) {
        return Some(center);
    }

    // 4) tiny fixed 4×4 grid inside bbox (16 probes, deterministic)
    let nx = 4usize;
    let ny = 4usize;
    let stepx = bx / ((nx as f64) + 1.0);
    let stepy = by / ((ny as f64) + 1.0);
    for ix in 1..=nx {
        for iy in 1..=ny {
            let p = Point::new(minx + stepx * ix as f64, miny + stepy * iy as f64);
            if poly.contains(&p) {
                return Some(p);
            }
        }
    }

    None
}
