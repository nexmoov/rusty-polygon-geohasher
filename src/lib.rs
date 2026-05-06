use geo::{
    algorithm::centroid::Centroid, Area, BoundingRect, Contains, InteriorPoint, Intersects, Point,
    Polygon, Rect,
};

use geohash::{decode_bbox, encode, neighbors, GeohashError};
use pyo3::prelude::*;
use pyo3::types::PyAny;
use pyo3::wrap_pyfunction;
use rayon::prelude::*;
use std::collections::{HashSet, VecDeque};

// ── Helpers ──────────────────────────────────────────────────────────────────

/// Build a custom thread pool, or return `None` to use the global Rayon pool.
///
/// Call this *before* releasing the GIL so that pool-creation errors can be
/// converted to Python exceptions while we still hold it.
fn make_pool(num_threads: Option<usize>) -> PyResult<Option<rayon::ThreadPool>> {
    match num_threads {
        None => Ok(None),
        Some(n) => rayon::ThreadPoolBuilder::new()
            .num_threads(n)
            .build()
            .map(Some)
            .map_err(|e| pyo3::exceptions::PyRuntimeError::new_err(e.to_string())),
    }
}

/// Run `f` on `pool`, or on the global Rayon pool if `pool` is `None`.
///
/// Call this *inside* `py.allow_threads` so the GIL is released while Rayon
/// workers are running.
fn run_with_pool<F, T>(pool: &Option<rayon::ThreadPool>, f: F) -> T
where
    F: FnOnce() -> T + Send,
    T: Send,
{
    match pool {
        None => f(),
        Some(p) => p.install(f),
    }
}

fn all_neighbors(hash: &str) -> Result<[String; 8], GeohashError> {
    let nbrs = neighbors(hash)?;
    Ok([nbrs.n, nbrs.ne, nbrs.e, nbrs.se, nbrs.s, nbrs.sw, nbrs.w, nbrs.nw])
}

/// BFS frontier expansion: expand a set of geohashes outward by `n_hops` steps.
/// Returns an error if any hash in the input set is malformed.
pub fn expand_geohash_set(
    geohashes: &HashSet<String>,
    n_hops: usize,
) -> Result<HashSet<String>, GeohashError> {
    let mut all = geohashes.clone();
    // Initial frontier: input cells with at least one neighbor outside the set.
    // Neighbour lookup here validates user-provided hashes.
    let mut frontier: HashSet<String> = HashSet::new();
    for gh in all.iter() {
        let nbrs = all_neighbors(gh)?;
        if nbrs.iter().any(|n| !all.contains(n)) {
            frontier.insert(gh.clone());
        }
    }
    for _ in 0..n_hops {
        let mut new_frontier: HashSet<String> = HashSet::new();
        for gh in &frontier {
            for n in all_neighbors(gh)? {
                if !all.contains(&n) {
                    new_frontier.insert(n);
                }
            }
        }
        all.extend(new_frontier.iter().cloned());
        frontier = new_frontier;
    }
    Ok(all)
}

// ── Polygon → geohash (existing) ─────────────────────────────────────────────

pub fn polygons_to_geohashes<PI>(
    polygons: PI,
    precision: usize,
    fully_contained_only: bool,
) -> Result<HashSet<String>, GeohashError>
where
    PI: IntoIterator<Item = Polygon>,
{
    let mut accepted_geohashes = HashSet::new();

    for polygon in polygons {
        // Reset per polygon: a cell rejected by one polygon in a multipolygon
        // must still be tested against the others.
        let mut rejected_geohashes = HashSet::new();
        let polygon_exterior = polygon.exterior();
        let has_holes = !polygon.interiors().is_empty();

        // choose a seed inside the polygon
        let Some(seed_point) = seed_interior_point_fast(&polygon) else {
            continue; // degenerate polygon, skip
        };

        // convert to geohash and start BFS
        let mut testing_geohashes = VecDeque::new();
        let seed_gh = encode((seed_point.x(), seed_point.y()).into(), precision)?;
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
                        testing_geohashes.push_back(neighbor.to_string());
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

/// Walk a `__geo_interface__` coordinate ring (list of [x, y] pairs) into a LineString.
fn extract_ring(ring: &Bound<'_, PyAny>) -> PyResult<geo_types::LineString<f64>> {
    let mut coords = Vec::new();
    for (i, item) in ring.try_iter()?.enumerate() {
        let pair = item?;
        let (x, y) = (|| -> PyResult<(f64, f64)> {
            Ok((pair.get_item(0)?.extract()?, pair.get_item(1)?.extract()?))
        })()
        .map_err(|_| {
            pyo3::exceptions::PyValueError::new_err(format!(
                "invalid coordinate at index {i}: expected [longitude, latitude]"
            ))
        })?;
        coords.push(geo_types::Coord { x, y });
    }
    Ok(geo_types::LineString::new(coords))
}

/// Build a `Polygon` from a `__geo_interface__` coordinates value (list of rings).
fn extract_polygon(coordinates: &Bound<'_, PyAny>) -> PyResult<Polygon<f64>> {
    let mut iter = coordinates.try_iter()?;
    let exterior = extract_ring(
        &iter
            .next()
            .ok_or_else(|| pyo3::exceptions::PyValueError::new_err("Polygon has no rings"))??,
    )?;
    let holes = iter
        .map(|r| -> PyResult<_> { extract_ring(&r?) })
        .collect::<PyResult<Vec<_>>>()?;
    Ok(Polygon::new(exterior, holes))
}

/// Build a `Vec<Polygon>` from a `__geo_interface__` MultiPolygon coordinates value.
fn extract_multipolygon(coordinates: &Bound<'_, PyAny>) -> PyResult<Vec<Polygon<f64>>> {
    coordinates
        .try_iter()?
        .map(|item| -> PyResult<_> { extract_polygon(&item?) })
        .collect()
}

#[pyfunction]
fn polygon_to_geohashes(
    _py: Python,
    py_polygon: Bound<'_, PyAny>,
    precision: usize,
    inner: bool,
) -> PyResult<HashSet<String>> {
    let geo_interface = py_polygon.getattr("__geo_interface__").map_err(|_| {
        pyo3::exceptions::PyValueError::new_err(
            "Object does not implement __geo_interface__. Expected a Shapely Polygon or MultiPolygon.",
        )
    })?;

    let geom_type: String = geo_interface
        .get_item("type")
        .map_err(|_| {
            pyo3::exceptions::PyValueError::new_err(
                "__geo_interface__ mapping is missing the required 'type' key",
            )
        })?
        .extract()
        .map_err(|_| {
            pyo3::exceptions::PyValueError::new_err(
                "__geo_interface__ 'type' value must be a string",
            )
        })?;

    let coordinates = geo_interface.get_item("coordinates").map_err(|_| {
        pyo3::exceptions::PyValueError::new_err(
            "__geo_interface__ mapping is missing the required 'coordinates' key",
        )
    })?;

    let polygons = match geom_type.as_str() {
        "Polygon" => vec![extract_polygon(&coordinates)?],
        "MultiPolygon" => extract_multipolygon(&coordinates)?,
        _ => {
            return Err(pyo3::exceptions::PyValueError::new_err(
                "The geometry is not a Polygon or MultiPolygon",
            ))
        }
    };

    polygons_to_geohashes(polygons, precision, inner)
        .map_err(|e| pyo3::exceptions::PyValueError::new_err(format!("{e:?}")))
}

// ── Encode / decode ───────────────────────────────────────────────────────────

/// Encode a single (lng, lat) coordinate to a geohash of the given precision.
#[pyfunction]
#[pyo3(name = "encode")]
fn encode_py(lng: f64, lat: f64, precision: usize) -> PyResult<String> {
    encode((lng, lat).into(), precision)
        .map_err(|e| pyo3::exceptions::PyValueError::new_err(e.to_string()))
}

/// Encode parallel lists of longitudes and latitudes to geohashes (parallel).
#[pyfunction]
#[pyo3(signature = (lngs, lats, precision, num_threads=None))]
fn encode_many(
    py: Python<'_>,
    lngs: Vec<f64>,
    lats: Vec<f64>,
    precision: usize,
    num_threads: Option<usize>,
) -> PyResult<Vec<String>> {
    if lngs.len() != lats.len() {
        return Err(pyo3::exceptions::PyValueError::new_err(
            "lngs and lats must have the same length",
        ));
    }
    let pool = make_pool(num_threads)?;
    let raw: Vec<Result<String, GeohashError>> = py.allow_threads(|| {
        run_with_pool(&pool, || {
            lngs.into_par_iter()
                .zip_eq(lats)
                .map(|(lng, lat)| encode((lng, lat).into(), precision))
                .collect()
        })
    });
    raw.into_iter()
        .map(|r| r.map_err(|e| pyo3::exceptions::PyValueError::new_err(e.to_string())))
        .collect()
}

/// Decode a geohash to (lng, lat, lng_err, lat_err) — lng-first, matching encode convention.
#[pyfunction]
fn decode_exactly(hash_str: &str) -> PyResult<(f64, f64, f64, f64)> {
    let bbox = decode_bbox(hash_str)
        .map_err(|e| pyo3::exceptions::PyValueError::new_err(e.to_string()))?;
    let lat = (bbox.min().y + bbox.max().y) / 2.0;
    let lng = (bbox.min().x + bbox.max().x) / 2.0;
    let lat_err = (bbox.max().y - bbox.min().y) / 2.0;
    let lng_err = (bbox.max().x - bbox.min().x) / 2.0;
    Ok((lng, lat, lng_err, lat_err))
}

/// Decode a list of geohashes to (lng, lat) center pairs (parallel).
#[pyfunction]
#[pyo3(signature = (geohashes, num_threads=None))]
fn decode_many(
    py: Python<'_>,
    geohashes: Vec<String>,
    num_threads: Option<usize>,
) -> PyResult<Vec<(f64, f64)>> {
    let pool = make_pool(num_threads)?;
    let raw: Vec<Result<(f64, f64), GeohashError>> = py.allow_threads(|| {
        run_with_pool(&pool, || {
            geohashes
                .into_par_iter()
                .map(|hash| {
                    decode_bbox(&hash).map(|bbox| {
                        let lat = (bbox.min().y + bbox.max().y) / 2.0;
                        let lng = (bbox.min().x + bbox.max().x) / 2.0;
                        (lng, lat)
                    })
                })
                .collect()
        })
    });
    raw.into_iter()
        .map(|r| r.map_err(|e| pyo3::exceptions::PyValueError::new_err(e.to_string())))
        .collect()
}

/// Decode a list of geohashes to (lng, lat, lng_err, lat_err) tuples (parallel).
#[pyfunction]
#[pyo3(signature = (geohashes, num_threads=None))]
fn decode_many_exactly(
    py: Python<'_>,
    geohashes: Vec<String>,
    num_threads: Option<usize>,
) -> PyResult<Vec<(f64, f64, f64, f64)>> {
    let pool = make_pool(num_threads)?;
    let raw: Vec<Result<(f64, f64, f64, f64), GeohashError>> = py.allow_threads(|| {
        run_with_pool(&pool, || {
            geohashes
                .into_par_iter()
                .map(|hash| {
                    decode_bbox(&hash).map(|bbox| {
                        let lat = (bbox.min().y + bbox.max().y) / 2.0;
                        let lng = (bbox.min().x + bbox.max().x) / 2.0;
                        let lat_err = (bbox.max().y - bbox.min().y) / 2.0;
                        let lng_err = (bbox.max().x - bbox.min().x) / 2.0;
                        (lng, lat, lng_err, lat_err)
                    })
                })
                .collect()
        })
    });
    raw.into_iter()
        .map(|r| r.map_err(|e| pyo3::exceptions::PyValueError::new_err(e.to_string())))
        .collect()
}

// ── Geography expansion ───────────────────────────────────────────────────────

fn n_hops_for(sample_hash: &str, expansion_m: f64) -> PyResult<usize> {
    if !expansion_m.is_finite() || expansion_m < 0.0 {
        return Err(pyo3::exceptions::PyValueError::new_err(
            "expansion_m must be a finite non-negative number",
        ));
    }
    let bbox = decode_bbox(sample_hash)
        .map_err(|e| pyo3::exceptions::PyValueError::new_err(format!("invalid geohash: {e}")))?;
    let lat_center = (bbox.min().y + bbox.max().y) / 2.0;
    let cell_height_m = (bbox.max().y - bbox.min().y) * 111_000.0;
    let cell_width_m = (bbox.max().x - bbox.min().x) * 111_320.0 * lat_center.to_radians().cos();
    let min_cell_m = cell_height_m.min(cell_width_m);
    Ok((expansion_m / min_cell_m).ceil() as usize)
}

/// Expand a single group of geohashes outward by `expansion_m` metres.
///
/// The hop count is derived from the cell height of a sample hash, so it works
/// for any precision level.
#[pyfunction]
fn expand_geohashes(py: Python<'_>, geohashes: Vec<String>, expansion_m: f64) -> PyResult<Vec<String>> {
    if geohashes.is_empty() {
        return Ok(vec![]);
    }
    let expected_len = geohashes.first().unwrap().len();
    if geohashes.iter().any(|h| h.len() != expected_len) {
        return Err(pyo3::exceptions::PyValueError::new_err(
            "all geohashes must have the same precision",
        ));
    }
    let n_hops = n_hops_for(geohashes.first().unwrap(), expansion_m)?;
    let hash_set: HashSet<String> = geohashes.into_iter().collect();
    py.allow_threads(|| expand_geohash_set(&hash_set, n_hops))
        .map(|s| s.into_iter().collect())
        .map_err(|e| pyo3::exceptions::PyValueError::new_err(e.to_string()))
}

/// Expand multiple groups of geohashes outward by `expansion_m` metres.
///
/// Each input group is expanded independently. Output order matches input order —
/// `result[i]` is the expanded version of `groups[i]`. Groups are processed in
/// parallel across geographies via Rayon.
///
/// The hop count is derived per group from the cell height of the group's first
/// hash, so groups at different precision levels are each handled correctly.
#[pyfunction]
fn expand_geohash_mapping(
    py: Python<'_>,
    groups: Vec<Vec<String>>,
    expansion_m: f64,
) -> PyResult<Vec<Vec<String>>> {
    if groups.is_empty() {
        return Ok(vec![]);
    }
    // Compute n_hops per group sequentially (fast, may raise PyErr) before releasing the GIL.
    let n_hops_per_group: Vec<usize> = groups
        .iter()
        .map(|g| match g.first() {
            Some(h) => {
                let expected_len = h.len();
                if g.iter().any(|gh| gh.len() != expected_len) {
                    return Err(pyo3::exceptions::PyValueError::new_err(
                        "all geohashes in a group must have the same precision",
                    ));
                }
                n_hops_for(h, expansion_m)
            }
            None => Ok(0),
        })
        .collect::<PyResult<_>>()?;

    let raw: Vec<Result<Vec<String>, GeohashError>> = py.allow_threads(|| {
        groups
            .into_par_iter()
            .zip(n_hops_per_group.into_par_iter())
            .map(|(hashes, n_hops)| {
                let hash_set: HashSet<String> = hashes.into_iter().collect();
                expand_geohash_set(&hash_set, n_hops).map(|s| s.into_iter().collect())
            })
            .collect()
    });
    raw.into_iter()
        .map(|r| r.map_err(|e| pyo3::exceptions::PyValueError::new_err(e.to_string())))
        .collect()
}

// ── Module ────────────────────────────────────────────────────────────────────

#[pymodule]
fn geohash_polygon(m: &Bound<'_, PyModule>) -> PyResult<()> {
    m.add_function(wrap_pyfunction!(polygon_to_geohashes, m)?)?;
    m.add_function(wrap_pyfunction!(encode_py, m)?)?;
    m.add_function(wrap_pyfunction!(encode_many, m)?)?;
    m.add_function(wrap_pyfunction!(decode_exactly, m)?)?;
    m.add_function(wrap_pyfunction!(decode_many, m)?)?;
    m.add_function(wrap_pyfunction!(decode_many_exactly, m)?)?;
    m.add_function(wrap_pyfunction!(expand_geohashes, m)?)?;
    m.add_function(wrap_pyfunction!(expand_geohash_mapping, m)?)?;
    Ok(())
}

// ── Interior seed (existing) ──────────────────────────────────────────────────

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

    // 5) guaranteed interior point from geo (handles thin/concave polygons where
    //    all fast probes fall inside the hollow region)
    poly.interior_point()
}
