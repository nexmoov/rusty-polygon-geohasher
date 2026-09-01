use geo::{
    algorithm::centroid::Centroid, BoundingRect, Contains, Intersects, Polygon, PreparedGeometry,
    Rect, Relate,
};

use arrow_array::{Array, ArrayRef, ListArray, RecordBatch, StringArray};
use arrow_array::builder::LargeStringBuilder;
use arrow_schema::{DataType, Field, Schema};
use geohash::{decode_bbox, encode, neighbors, GeohashError};
use pyo3::prelude::*;
use pyo3::types::PyAny;
use pyo3::wrap_pyfunction;
use rayon::prelude::*;
use rustc_hash::FxHashSet;
use std::collections::{HashSet, VecDeque};
use std::sync::Arc;

// ── Integer geohash representation ───────────────────────────────────────────

/// A geohash of precision `p` is `5 * p` interleaved bits, so every hash of
/// precision 1..=12 (the range the `geohash` crate accepts) fits in a `u64`.
///
/// Working on the packed integer instead of the `String` removes the eight heap
/// allocations per cell that `geohash::neighbors` costs, lets cells live in a
/// `FxHashSet<u64>`, and makes a cell's bounding box a couple of shifts and
/// multiplies rather than a base32 decode.
///
/// Bit layout matches the geohash spec: the most significant bit is longitude,
/// and longitude/latitude alternate from there. With `n = 5 * p` bits total,
/// longitude therefore occupies `ceil(n / 2)` bits and latitude `floor(n / 2)`.
mod ghbits {
    use geo::Rect;

    pub const BASE32: &[u8; 32] = b"0123456789bcdefghjkmnpqrstuvwxyz";
    pub const MAX_PRECISION: usize = 12;

    /// Gather the even-indexed bits of `x` into the low half of the result.
    #[inline]
    fn compact_even(mut x: u64) -> u64 {
        x &= 0x5555_5555_5555_5555;
        x = (x | (x >> 1)) & 0x3333_3333_3333_3333;
        x = (x | (x >> 2)) & 0x0f0f_0f0f_0f0f_0f0f;
        x = (x | (x >> 4)) & 0x00ff_00ff_00ff_00ff;
        x = (x | (x >> 8)) & 0x0000_ffff_0000_ffff;
        (x | (x >> 16)) & 0x0000_0000_ffff_ffff
    }

    /// Inverse of [`compact_even`]: scatter the low 32 bits of `x` to even positions.
    #[inline]
    fn spread_even(mut x: u64) -> u64 {
        x &= 0x0000_0000_ffff_ffff;
        x = (x | (x << 16)) & 0x0000_ffff_0000_ffff;
        x = (x | (x << 8)) & 0x00ff_00ff_00ff_00ff;
        x = (x | (x << 4)) & 0x0f0f_0f0f_0f0f_0f0f;
        x = (x | (x << 2)) & 0x3333_3333_3333_3333;
        (x | (x << 1)) & 0x5555_5555_5555_5555
    }

    /// Number of longitude and latitude bits at `precision`.
    #[inline]
    pub fn axis_bits(precision: usize) -> (u32, u32) {
        let n = 5 * precision;
        (n.div_ceil(2) as u32, (n / 2) as u32)
    }

    /// Split a packed hash into its (longitude, latitude) grid indices.
    #[inline]
    pub fn split(v: u64, precision: usize) -> (u64, u64) {
        // Bit `j` counted from the LSB is a longitude bit iff `n - 1 - j` is even,
        // i.e. iff `j` has the same parity as `n - 1`.
        if (5 * precision) % 2 == 1 {
            (compact_even(v), compact_even(v >> 1))
        } else {
            (compact_even(v >> 1), compact_even(v))
        }
    }

    /// Interleave (longitude, latitude) grid indices back into a packed hash.
    #[inline]
    pub fn merge(lon: u64, lat: u64, precision: usize) -> u64 {
        if (5 * precision) % 2 == 1 {
            spread_even(lon) | (spread_even(lat) << 1)
        } else {
            (spread_even(lon) << 1) | spread_even(lat)
        }
    }

    /// Parse a base32 geohash into its packed form.
    ///
    /// `None` if any character is outside the geohash alphabet (which omits
    /// 'a', 'i', 'l' and 'o'), or if the hash is empty or longer than
    /// [`MAX_PRECISION`]. Past that length the five-bit shifts below would run
    /// off the top of the `u64` and silently alias distinct hashes onto the
    /// same packed value.
    pub fn pack(hash: &str) -> Option<u64> {
        if !(1..=MAX_PRECISION).contains(&hash.len()) {
            return None;
        }
        let mut v = 0u64;
        for c in hash.bytes() {
            let idx = BASE32.iter().position(|&b| b == c)?;
            v = (v << 5) | idx as u64;
        }
        Some(v)
    }

    /// Render a packed hash back to base32.
    pub fn unpack(v: u64, precision: usize) -> String {
        let mut buf = vec![0u8; precision];
        for (i, slot) in buf.iter_mut().enumerate().rev() {
            *slot = BASE32[((v >> (5 * (precision - 1 - i))) & 31) as usize];
        }
        // Every byte came from BASE32, which is ASCII.
        String::from_utf8(buf).expect("base32 alphabet is ASCII")
    }

    /// Bounding box of a packed hash, straight from its grid indices.
    #[inline]
    pub fn bbox(v: u64, precision: usize) -> Rect<f64> {
        let (lon_bits, lat_bits) = axis_bits(precision);
        let (lon_i, lat_i) = split(v, precision);
        let lon_span = 360.0 / (1u64 << lon_bits) as f64;
        let lat_span = 180.0 / (1u64 << lat_bits) as f64;
        let xmin = -180.0 + lon_i as f64 * lon_span;
        let ymin = -90.0 + lat_i as f64 * lat_span;
        Rect::new((xmin, ymin), (xmin + lon_span, ymin + lat_span))
    }

    /// Write the neighbours of `v` into `out`, returning how many were written.
    ///
    /// Longitude wraps at the antimeridian. Latitude does *not*: there is no cell
    /// north of the top row, so polar cells have five neighbours rather than
    /// eight. `geohash::neighbors` disagrees here and wraps over the pole to the
    /// opposite edge of the grid, which would teleport a polar expansion into the
    /// other hemisphere.
    #[inline]
    pub fn neighbors(v: u64, precision: usize, out: &mut [u64; 8]) -> usize {
        let (lon_bits, lat_bits) = axis_bits(precision);
        let (lon_i, lat_i) = split(v, precision);
        let lon_mask = (1u64 << lon_bits) - 1;
        let lat_max = (1u64 << lat_bits) - 1;
        let mut n = 0;
        for dy in [-1i64, 0, 1] {
            for dx in [-1i64, 0, 1] {
                if dx == 0 && dy == 0 {
                    continue;
                }
                let ny = lat_i as i64 + dy;
                if ny < 0 || ny as u64 > lat_max {
                    continue;
                }
                let nx = (lon_i.wrapping_add(dx as u64)) & lon_mask;
                out[n] = merge(nx, ny as u64, precision);
                n += 1;
            }
        }
        n
    }
}

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
/// Call this *inside* `py.detach` so the GIL is released while Rayon
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

/// Locate the character that stopped [`ghbits::pack`], for the error path.
fn invalid_hash_error(hash: &str) -> GeohashError {
    let bad = hash
        .chars()
        .find(|c| !c.is_ascii() || !ghbits::BASE32.contains(&(*c as u8)))
        .unwrap_or('?');
    GeohashError::InvalidHashCharacter(bad)
}

/// Expand a set of geohashes outward by `n_hops` rings of neighbouring cells.
///
/// Every hash must share one precision, since the cells are walked on that
/// precision's grid. Returns an error if any hash is malformed or the set mixes
/// precisions.
pub fn expand_geohash_set(
    geohashes: &HashSet<String>,
    n_hops: usize,
) -> Result<HashSet<String>, GeohashError> {
    let Some(first) = geohashes.iter().next() else {
        return Ok(HashSet::new());
    };
    let precision = first.len();
    if !(1..=ghbits::MAX_PRECISION).contains(&precision) {
        return Err(GeohashError::InvalidLength(precision));
    }

    // Packing validates every input hash, so this runs even for a zero-hop
    // expansion where the answer is just the input back.
    let mut all: FxHashSet<u64> =
        FxHashSet::with_capacity_and_hasher(geohashes.len(), Default::default());
    for hash in geohashes {
        if hash.len() != precision {
            return Err(GeohashError::InvalidLength(hash.len()));
        }
        all.insert(ghbits::pack(hash).ok_or_else(|| invalid_hash_error(hash))?);
    }

    if n_hops == 0 {
        return Ok(geohashes.clone());
    }

    // Ring by ring. Cells already in `all` are never revisited, so interior cells
    // drop out of the frontier after the first pass without a separate boundary
    // scan.
    let mut frontier: Vec<u64> = all.iter().copied().collect();
    let mut next: Vec<u64> = Vec::new();
    let mut buf = [0u64; 8];
    for _ in 0..n_hops {
        next.clear();
        for &cell in &frontier {
            let count = ghbits::neighbors(cell, precision, &mut buf);
            for &neighbor in &buf[..count] {
                if all.insert(neighbor) {
                    next.push(neighbor);
                }
            }
        }
        if next.is_empty() {
            break; // the grid is saturated; further hops cannot add anything
        }
        std::mem::swap(&mut frontier, &mut next);
    }

    // The final count is already known, so the output set never rehashes.
    let mut out = HashSet::with_capacity(all.len());
    out.extend(all.into_iter().map(|cell| ghbits::unpack(cell, precision)));
    Ok(out)
}

// ── Polygon → geohash ────────────────────────────────────────────────────────

/// Insert `cell` and every one of its descendants at `precision`.
///
/// Called once a cell is known to lie wholly inside the polygon, at which point
/// its whole subtree is inside too and needs no further geometry tests.
#[inline]
fn emit_subtree(cell: u64, level: usize, precision: usize, out: &mut FxHashSet<u64>) {
    let shift = 5 * (precision - level);
    let base = cell << shift;
    for k in 0..(1u64 << shift) {
        out.insert(base | k);
    }
}

/// Number of cells at `level` needed to cover `bbox`.
fn cover_count(bbox: &Rect<f64>, level: usize) -> u64 {
    let (i0, i1, j0, j1) = cover_range(bbox, level);
    (i1 - i0 + 1) * (j1 - j0 + 1)
}

/// Inclusive grid-index range `(lon_min, lon_max, lat_min, lat_max)` covering `bbox`.
fn cover_range(bbox: &Rect<f64>, level: usize) -> (u64, u64, u64, u64) {
    let (lon_bits, lat_bits) = ghbits::axis_bits(level);
    let lon_span = 360.0 / (1u64 << lon_bits) as f64;
    let lat_span = 180.0 / (1u64 << lat_bits) as f64;
    let lon_max = (1u64 << lon_bits) - 1;
    let lat_max = (1u64 << lat_bits) - 1;
    let clamp = |raw: f64, max: u64| -> u64 {
        if raw < 0.0 {
            0
        } else {
            (raw as u64).min(max)
        }
    };
    // The upper index uses `floor`, so a maximum sitting exactly on a grid line
    // still names the cell east/north of it — the one the box touches along that
    // line. `ceil(v / span) - 1` is the mirror of that for the lower index: it
    // agrees with `floor` off the grid lines and steps one cell west/south on
    // them, so a box flush against a grid line seeds the touching cell on both
    // sides. `relate` still decides whether each seeded cell is kept, so the
    // only cost of an extra seed is one topological test.
    let lo = |v: f64, span: f64, max: u64| -> u64 { clamp((v / span).ceil() - 1.0, max) };
    let hi = |v: f64, span: f64, max: u64| -> u64 { clamp((v / span).floor(), max) };
    (
        lo(bbox.min().x + 180.0, lon_span, lon_max),
        hi(bbox.max().x + 180.0, lon_span, lon_max),
        lo(bbox.min().y + 90.0, lat_span, lat_max),
        hi(bbox.max().y + 90.0, lat_span, lat_max),
    )
}

/// Seed the descent at the deepest level whose cover is still small.
///
/// Starting at precision 1 would spend a `relate` call per cell on levels where
/// the cell dwarfs the polygon — and at those sizes the R*-tree cannot prune, so
/// each call walks every edge. Skipping straight to a level whose cells are
/// comparable to the polygon avoids that entirely.
fn descent_start(bbox: &Rect<f64>, precision: usize) -> (Vec<(u64, usize)>, usize) {
    const MAX_START_CELLS: u64 = 64;

    // Level 1 has only 32 cells in total, so it always satisfies the bound.
    let mut level = 1;
    while level < precision && cover_count(bbox, level + 1) <= MAX_START_CELLS {
        level += 1;
    }

    let (i0, i1, j0, j1) = cover_range(bbox, level);
    let mut cells = Vec::with_capacity(((i1 - i0 + 1) * (j1 - j0 + 1)) as usize);
    for i in i0..=i1 {
        for j in j0..=j1 {
            cells.push((ghbits::merge(i, j, level), level));
        }
    }
    (cells, level)
}

/// The corner of `bbox` that lies outside the geohash domain, if any.
///
/// `cover_range` clamps every index onto the grid, so geometry beyond
/// [-180, 180] x [-90, 90] would otherwise come back as a silently empty or
/// truncated cover instead of an error.
fn out_of_range_corner(bbox: &Rect<f64>) -> Option<geo_types::Coord<f64>> {
    [bbox.min(), bbox.max()]
        .into_iter()
        .find(|c| !(-180.0..=180.0).contains(&c.x) || !(-90.0..=90.0).contains(&c.y))
}

/// Cells of `precision` covering `polygons`, either intersecting them or wholly
/// inside them when `fully_contained_only` is set.
///
/// Walks the geohash tree top-down rather than flood-filling at `precision`: a
/// cell disjoint from the polygon prunes its entire subtree, and a cell the
/// polygon contains yields all its descendants without further geometry tests.
/// Only cells straddling the boundary are subdivided, so cost tracks the
/// polygon's perimeter instead of its area.
///
/// A single [`PreparedGeometry::relate`] per visited cell answers "intersects"
/// and "contains" together, and its R*-tree keeps each call proportional to the
/// edges near that cell rather than the polygon's full vertex count.
pub fn polygons_to_geohashes<PI>(
    polygons: PI,
    precision: usize,
    fully_contained_only: bool,
) -> Result<HashSet<String>, GeohashError>
where
    PI: IntoIterator<Item = Polygon>,
{
    if !(1..=ghbits::MAX_PRECISION).contains(&precision) {
        return Err(GeohashError::InvalidLength(precision));
    }

    let mut accepted: FxHashSet<u64> = FxHashSet::default();

    for polygon in polygons {
        let Some(polygon_bbox) = polygon.bounding_rect() else {
            continue; // empty polygon, nothing to cover
        };
        if let Some(corner) = out_of_range_corner(&polygon_bbox) {
            return Err(GeohashError::InvalidCoordinateRange(corner));
        }
        let prepared: PreparedGeometry<_> = PreparedGeometry::from(&polygon);

        let (mut stack, _) = descent_start(&polygon_bbox, precision);
        while let Some((cell, level)) = stack.pop() {
            let cell_bbox = ghbits::bbox(cell, level);

            // Cheap rejection before paying for a topological test.
            if !polygon_bbox.intersects(&cell_bbox) {
                continue;
            }

            let relation = prepared.relate(&cell_bbox);
            if !relation.is_intersects() {
                continue; // whole subtree is outside
            }
            if relation.is_contains() {
                emit_subtree(cell, level, precision, &mut accepted); // whole subtree is inside
                continue;
            }
            if level == precision {
                // Straddles the boundary and cannot be subdivided further.
                if !fully_contained_only {
                    accepted.insert(cell);
                }
                continue;
            }

            let base = cell << 5;
            for child in 0..32u64 {
                stack.push((base | child, level + 1));
            }
        }
    }

    Ok(accepted
        .into_iter()
        .map(|cell| ghbits::unpack(cell, precision))
        .collect())
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
    py: Python<'_>,
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

    // The cover walk touches no Python objects, and at fine precisions it runs for
    // seconds — verdun at precision 10 takes minutes. Hold the GIL for the
    // __geo_interface__ read above only, not for the geometry.
    py.detach(|| polygons_to_geohashes(polygons, precision, inner))
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
    let raw: Vec<Result<String, GeohashError>> = py.detach(|| {
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
    let raw: Vec<Result<(f64, f64), GeohashError>> = py.detach(|| {
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
    let raw: Vec<Result<(f64, f64, f64, f64), GeohashError>> = py.detach(|| {
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

/// Serialize a bounding box as a little-endian WKB or EWKB polygon (1 ring, 5 points, closed).
///
/// Pass `srid: None` for plain WKB (93 bytes). Pass `srid: Some(s)` for EWKB (97 bytes),
/// which sets the SRID flag (0x20000000) in the type field and inserts a 4-byte SRID.
#[inline]
fn serialize_bbox(xmin: f64, ymin: f64, xmax: f64, ymax: f64, srid: Option<u32>) -> Vec<u8> {
    let capacity = if srid.is_some() { 97 } else { 93 };
    let wkb_type = if srid.is_some() { 3u32 | 0x20000000u32 } else { 3u32 };
    let mut buf = Vec::with_capacity(capacity);
    buf.push(0x01u8);
    buf.extend_from_slice(&wkb_type.to_le_bytes());
    if let Some(s) = srid {
        buf.extend_from_slice(&s.to_le_bytes());
    }
    buf.extend_from_slice(&1u32.to_le_bytes()); // number of rings
    buf.extend_from_slice(&5u32.to_le_bytes()); // number of points (closed ring)
    for (x, y) in [
        (xmin, ymin),
        (xmax, ymin),
        (xmax, ymax),
        (xmin, ymax),
        (xmin, ymin), // close the ring
    ] {
        buf.extend_from_slice(&x.to_le_bytes());
        buf.extend_from_slice(&y.to_le_bytes());
    }
    buf
}

fn geohashes_to_bytes(
    geohashes: Vec<String>,
    srid: Option<u32>,
    pool: &Option<rayon::ThreadPool>,
) -> Vec<Result<Vec<u8>, GeohashError>> {
    run_with_pool(pool, || {
        geohashes
            .into_par_iter()
            .map(|hash| {
                decode_bbox(&hash).map(|bbox| {
                    serialize_bbox(bbox.min().x, bbox.min().y, bbox.max().x, bbox.max().y, srid)
                })
            })
            .collect()
    })
}

fn into_py_wkb_results(raw: Vec<Result<Vec<u8>, GeohashError>>) -> PyResult<Vec<Vec<u8>>> {
    raw.into_iter()
        .map(|r| r.map_err(|e| pyo3::exceptions::PyValueError::new_err(e.to_string())))
        .collect()
}

/// Parallel Rust core of `decode_many_to_wkb`, without PyO3 overhead.
pub fn geohashes_to_wkb(
    geohashes: Vec<String>,
    pool: &Option<rayon::ThreadPool>,
) -> Vec<Result<Vec<u8>, GeohashError>> {
    geohashes_to_bytes(geohashes, None, pool)
}

/// Decode a list of geohashes to WKB polygon bytes representing their bounding boxes (parallel).
///
/// Each returned bytes value is a standard little-endian WKB polygon with one ring of five
/// points (closed bounding box). Pass the result to `ST_GeomFromWKB` in DuckDB or PostGIS.
#[pyfunction]
#[pyo3(signature = (geohashes, num_threads=None))]
fn decode_many_to_wkb(
    py: Python<'_>,
    geohashes: Vec<String>,
    num_threads: Option<usize>,
) -> PyResult<Vec<Vec<u8>>> {
    let pool = make_pool(num_threads)?;
    into_py_wkb_results(py.detach(|| geohashes_to_wkb(geohashes, &pool)))
}

/// Parallel Rust core of `decode_many_to_ewkb`, without PyO3 overhead.
pub fn geohashes_to_ewkb(
    geohashes: Vec<String>,
    srid: u32,
    pool: &Option<rayon::ThreadPool>,
) -> Vec<Result<Vec<u8>, GeohashError>> {
    geohashes_to_bytes(geohashes, Some(srid), pool)
}

/// Decode a list of geohashes to EWKB polygon bytes with an embedded SRID (parallel).
///
/// Like `decode_many_to_wkb` but with a SRID embedded in the header, making the
/// bytes suitable for direct insertion into PostGIS geometry columns without a
/// separate `ST_SetSRID` call. `srid` defaults to 4326.
#[pyfunction]
#[pyo3(signature = (geohashes, srid=4326, num_threads=None))]
fn decode_many_to_ewkb(
    py: Python<'_>,
    geohashes: Vec<String>,
    srid: u32,
    num_threads: Option<usize>,
) -> PyResult<Vec<Vec<u8>>> {
    let pool = make_pool(num_threads)?;
    into_py_wkb_results(py.detach(|| geohashes_to_ewkb(geohashes, srid, &pool)))
}

// ── Geography expansion ───────────────────────────────────────────────────────

/// Upper bound on the hop count `n_hops_for` will return.
///
/// Each hop grows the frontier by a ring, so a blob expanded by `n` hops gains
/// on the order of `n^2` cells. Ten thousand hops is already far past anything
/// a real expansion needs, and well short of the counts a degenerate input can
/// produce, so it separates "expensive" from "will never finish".
const MAX_EXPANSION_HOPS: usize = 10_000;

/// Hops needed to cover `expansion_m` metres, given a sample cell's dimensions.
///
/// Uses the smaller of the cell's height and width so the expansion reaches at
/// least `expansion_m` in every direction. Cell width shrinks with
/// `cos(latitude)`, so the hop count climbs steeply toward the poles: at
/// precision 9, expanding 1 km needs 419 hops at latitude 60 but 1,206 at
/// latitude 80 and around 1.5e8 at the top row. Without the cap below, a polar
/// input would build a frontier of billions of cells and never return.
fn n_hops_for_core(sample_hash: &str, expansion_m: f64) -> Result<usize, String> {
    if !expansion_m.is_finite() || expansion_m < 0.0 {
        return Err("expansion_m must be a finite non-negative number".to_string());
    }
    let bbox = decode_bbox(sample_hash).map_err(|e| format!("invalid geohash: {e}"))?;
    let lat_center = (bbox.min().y + bbox.max().y) / 2.0;
    let cell_height_m = (bbox.max().y - bbox.min().y) * 111_000.0;
    let cell_width_m = (bbox.max().x - bbox.min().x) * 111_320.0 * lat_center.to_radians().cos();
    let min_cell_m = cell_height_m.min(cell_width_m);

    if !min_cell_m.is_finite() || min_cell_m <= 0.0 {
        return Err(format!(
            "cannot size an expansion against geohash {sample_hash:?}: its cell has no \
             usable extent (width {cell_width_m} m, height {cell_height_m} m)"
        ));
    }

    let hops = (expansion_m / min_cell_m).ceil();
    if hops > MAX_EXPANSION_HOPS as f64 {
        return Err(format!(
            "expanding by {expansion_m} m from geohash {sample_hash:?} needs {hops:.0} hops, \
             over the limit of {MAX_EXPANSION_HOPS}. The cell is only {min_cell_m:.3} m across \
             at its narrowest — use a coarser precision, or a smaller expansion_m."
        ));
    }
    Ok(hops as usize)
}

/// Hop count for a whole group: the largest count any of its cells needs.
///
/// Cell width shrinks with `cos(latitude)`, so a count sized on one sampled
/// cell under-reaches every cell closer to a pole than the sample — and which
/// cell got sampled depended on input order. Sizing on the narrowest cell in
/// the group keeps the "at least `expansion_m` in every direction" promise for
/// every member; the wider cells merely reach a little further.
fn n_hops_for_group_core<'a, I>(hashes: I, expansion_m: f64) -> Result<usize, String>
where
    I: IntoIterator<Item = &'a str>,
{
    let mut hops = 0;
    for hash in hashes {
        hops = hops.max(n_hops_for_core(hash, expansion_m)?);
    }
    Ok(hops)
}

/// [`n_hops_for_group_core`] with its failure surfaced as a Python `ValueError`.
fn n_hops_for_group<'a, I>(hashes: I, expansion_m: f64) -> PyResult<usize>
where
    I: IntoIterator<Item = &'a str>,
{
    n_hops_for_group_core(hashes, expansion_m).map_err(pyo3::exceptions::PyValueError::new_err)
}

/// Expand a single group of geohashes outward by `expansion_m` metres.
///
/// The hop count is sized on the narrowest cell in the group, so the expansion
/// reaches at least `expansion_m` in every direction from every cell at any
/// precision level.
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
    let n_hops = n_hops_for_group(geohashes.iter().map(String::as_str), expansion_m)?;
    let hash_set: HashSet<String> = geohashes.into_iter().collect();
    py.detach(|| expand_geohash_set(&hash_set, n_hops))
        .map(|s| s.into_iter().collect())
        .map_err(|e| pyo3::exceptions::PyValueError::new_err(e.to_string()))
}

/// Expand multiple groups of geohashes outward by `expansion_m` metres.
///
/// Each input group is expanded independently. Output order matches input order —
/// `result[i]` is the expanded version of `groups[i]`. Groups are processed in
/// parallel across geographies via Rayon.
///
/// The hop count is sized per group on its narrowest cell, so groups at
/// different precision levels or latitudes are each handled correctly.
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
                n_hops_for_group(g.iter().map(String::as_str), expansion_m)
            }
            None => Ok(0),
        })
        .collect::<PyResult<_>>()?;

    let raw: Vec<Result<Vec<String>, GeohashError>> = py.detach(|| {
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

/// Expand multiple groups of geohashes outward by `expansion_m` metres, passing data as Arrow
/// arrays to avoid Python object allocation at the boundary.
///
/// Accepts two PyArrow `Array` objects (not ChunkedArray — call `.combine_chunks()` first):
///   - `geog_ids`: a Utf8 `StringArray` of N geog_id strings
///   - `geohash_lists`: a `List<Utf8>` array where element `i` holds the geohashes for geog `i`
///
/// Returns a flat PyArrow `RecordBatch` with schema `(geog_id: LargeUtf8, geohash: LargeUtf8)` —
/// one row per (geog_id, expanded_geohash) pair, with geog_ids repeated as needed.
///
/// Compared to `expand_geohash_mapping`, this function eliminates the Python str object
/// round-trip: strings are read directly from Arrow buffers and the output is built into
/// Arrow buffers without touching the Python heap at all.
#[pyfunction]
fn expand_geohash_mapping_arrow(
    py: Python<'_>,
    geog_ids: pyo3_arrow::PyArray,
    geohash_lists: pyo3_arrow::PyArray,
    expansion_m: f64,
) -> PyResult<pyo3_arrow::PyRecordBatch> {
    if !expansion_m.is_finite() || expansion_m < 0.0 {
        return Err(pyo3::exceptions::PyValueError::new_err(
            "expansion_m must be a finite non-negative number",
        ));
    }

    let (geog_id_ref, _) = geog_ids.into_inner();
    let (geohash_list_ref, _) = geohash_lists.into_inner();

    let geog_id_arr = geog_id_ref
        .as_any()
        .downcast_ref::<StringArray>()
        .ok_or_else(|| pyo3::exceptions::PyValueError::new_err("geog_ids must be a Utf8 Array"))?;

    let list_arr = geohash_list_ref
        .as_any()
        .downcast_ref::<ListArray>()
        .ok_or_else(|| {
            pyo3::exceptions::PyValueError::new_err("geohash_lists must be a List<Utf8> Array")
        })?;

    let values_arr = list_arr
        .values()
        .as_any()
        .downcast_ref::<StringArray>()
        .ok_or_else(|| {
            pyo3::exceptions::PyValueError::new_err("geohash list element type must be Utf8")
        })?;

    let n = geog_id_arr.len();
    if list_arr.len() != n {
        return Err(pyo3::exceptions::PyValueError::new_err(
            "geog_ids and geohash_lists must have the same length",
        ));
    }
    if let Some(i) = (0..n).find(|&i| geog_id_arr.is_null(i)) {
        return Err(pyo3::exceptions::PyValueError::new_err(format!(
            "geog_ids contains a null at index {i}"
        )));
    }
    let offsets = list_arr.offsets();

    // Compute n_hops per group while holding the GIL (n_hops_for can raise PyErr).
    let n_hops_per_group: Vec<usize> = (0..n)
        .map(|i| {
            let start = offsets[i] as usize;
            let end = offsets[i + 1] as usize;
            if start == end {
                return Ok(0);
            }
            let first = values_arr.value(start);
            let expected_len = first.len();
            if (start + 1..end).any(|j| values_arr.value(j).len() != expected_len) {
                return Err(pyo3::exceptions::PyValueError::new_err(
                    "all geohashes in a group must have the same precision",
                ));
            }
            n_hops_for_group((start..end).map(|j| values_arr.value(j)), expansion_m)
        })
        .collect::<PyResult<_>>()?;

    // Clone strings from Arrow buffers into owned Strings. Reading from Arrow's UTF-8
    // buffer is ~5× faster than going through to_pylist() + PyO3 str conversion, since
    // it skips the CPython object layer entirely.
    let groups: Vec<(String, Vec<String>, usize)> = (0..n)
        .map(|i| {
            let geog_id = geog_id_arr.value(i).to_owned();
            let start = offsets[i] as usize;
            let end = offsets[i + 1] as usize;
            let hashes: Vec<String> =
                (start..end).map(|j| values_arr.value(j).to_owned()).collect();
            (geog_id, hashes, n_hops_per_group[i])
        })
        .collect();

    // Release the GIL and expand all groups in parallel via Rayon.
    let results: Vec<(String, Result<HashSet<String>, GeohashError>)> =
        py.detach(|| {
            groups
                .into_par_iter()
                .map(|(geog_id, hashes, n_hops)| {
                    let hash_set: HashSet<String> = hashes.into_iter().collect();
                    (geog_id, expand_geohash_set(&hash_set, n_hops))
                })
                .collect()
        });

    // Count output rows so we can pre-allocate Arrow builders exactly once.
    let total_out: usize = results
        .iter()
        .map(|(_, r)| r.as_ref().map_or(0, |s| s.len()))
        .sum();

    // Build flat Arrow output directly in Rust — no Python str objects, no flatten loop,
    // no pa.array() round-trip.
    let mut out_geog_ids = LargeStringBuilder::with_capacity(total_out, total_out * 20);
    let mut out_geohashes = LargeStringBuilder::with_capacity(total_out, total_out * 7);

    for (geog_id, expanded) in results {
        let expanded =
            expanded.map_err(|e| pyo3::exceptions::PyValueError::new_err(e.to_string()))?;
        for geohash in expanded {
            out_geog_ids.append_value(&geog_id);
            out_geohashes.append_value(&geohash);
        }
    }

    let schema = Arc::new(Schema::new(vec![
        Field::new("geog_id", DataType::LargeUtf8, false),
        Field::new("geohash", DataType::LargeUtf8, false),
    ]));

    let columns: Vec<ArrayRef> = vec![
        Arc::new(out_geog_ids.finish()),
        Arc::new(out_geohashes.finish()),
    ];

    let batch = RecordBatch::try_new(schema, columns)
        .map_err(|e| pyo3::exceptions::PyValueError::new_err(e.to_string()))?;

    Ok(pyo3_arrow::PyRecordBatch::new(batch))
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
    m.add_function(wrap_pyfunction!(decode_many_to_wkb, m)?)?;
    m.add_function(wrap_pyfunction!(decode_many_to_ewkb, m)?)?;
    m.add_function(wrap_pyfunction!(expand_geohashes, m)?)?;
    m.add_function(wrap_pyfunction!(expand_geohash_mapping, m)?)?;
    m.add_function(wrap_pyfunction!(expand_geohash_mapping_arrow, m)?)?;
    Ok(())
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use geohash::decode_bbox;

    fn read_f64_le(buf: &[u8], offset: usize) -> f64 {
        f64::from_le_bytes(buf[offset..offset + 8].try_into().unwrap())
    }

    /// Parse a WKB or EWKB bbox polygon and return (xmin, ymin, xmax, ymax).
    /// Pass `srid: None` for plain WKB, `srid: Some(s)` to also assert the embedded SRID.
    fn parse_polygon_bbox(buf: &[u8], srid: Option<u32>) -> (f64, f64, f64, f64) {
        let has_srid = srid.is_some();
        let expected_len = if has_srid { 97 } else { 93 };
        let expected_type = if has_srid { 3u32 | 0x20000000u32 } else { 3u32 };
        // header: byte_order(1) + type(4) [+ srid(4)] + rings(4) + points(4)
        let coord_offset = if has_srid { 17usize } else { 13usize };
        assert_eq!(buf.len(), expected_len);
        assert_eq!(buf[0], 0x01, "byte order must be little-endian");
        assert_eq!(u32::from_le_bytes(buf[1..5].try_into().unwrap()), expected_type, "WKB type mismatch");
        let mut offset = 5;
        if let Some(expected_srid) = srid {
            assert_eq!(u32::from_le_bytes(buf[offset..offset + 4].try_into().unwrap()), expected_srid, "SRID mismatch");
            offset += 4;
        }
        assert_eq!(u32::from_le_bytes(buf[offset..offset + 4].try_into().unwrap()), 1, "ring count must be 1");
        assert_eq!(u32::from_le_bytes(buf[offset + 4..offset + 8].try_into().unwrap()), 5, "point count must be 5");
        // Points: (xmin,ymin), (xmax,ymin), (xmax,ymax), (xmin,ymax), (xmin,ymin)
        (
            read_f64_le(buf, coord_offset),      // xmin — point 0 x
            read_f64_le(buf, coord_offset + 8),  // ymin — point 0 y
            read_f64_le(buf, coord_offset + 16), // xmax — point 1 x
            read_f64_le(buf, coord_offset + 40), // ymax — point 2 y
        )
    }

    // ── serialize_bbox (WKB) ─────────────────────────────────────────────────

    #[test]
    fn test_bbox_to_wkb_is_93_bytes() {
        assert_eq!(serialize_bbox(0.0, 0.0, 1.0, 1.0, None).len(), 93);
    }

    #[test]
    fn test_bbox_to_wkb_header() {
        parse_polygon_bbox(&serialize_bbox(0.0, 0.0, 1.0, 1.0, None), None);
    }

    #[test]
    fn test_bbox_to_wkb_coordinates() {
        let (xmin, ymin, xmax, ymax) = (-73.5853_f64, 45.5017, -73.5702, 45.5098);
        let (gx1, gy1, gx2, gy2) = parse_polygon_bbox(&serialize_bbox(xmin, ymin, xmax, ymax, None), None);
        assert_eq!(gx1, xmin);
        assert_eq!(gy1, ymin);
        assert_eq!(gx2, xmax);
        assert_eq!(gy2, ymax);
    }

    #[test]
    fn test_bbox_to_wkb_ring_is_closed() {
        let (xmin, ymin) = (-73.5853_f64, 45.5017_f64);
        let wkb = serialize_bbox(xmin, ymin, -73.5702, 45.5098, None);
        assert_eq!(read_f64_le(&wkb, 77), xmin); // point 4 x
        assert_eq!(read_f64_le(&wkb, 85), ymin); // point 4 y
    }

    // ── geohashes_to_wkb (pure Rust core) ────────────────────────────────────

    fn run(geohashes: Vec<&str>, num_threads: Option<usize>) -> Vec<Result<Vec<u8>, GeohashError>> {
        let pool = num_threads.map(|n| rayon::ThreadPoolBuilder::new().num_threads(n).build().unwrap());
        geohashes_to_wkb(geohashes.into_iter().map(String::from).collect(), &pool)
    }

    #[test]
    fn test_geohashes_to_wkb_empty() {
        assert!(run(vec![], None).is_empty());
    }

    #[test]
    fn test_geohashes_to_wkb_single() {
        let results = run(vec!["dpz8zzzz"], None);
        assert_eq!(results.len(), 1);
        let wkb = results.into_iter().next().unwrap().unwrap();
        let expected = decode_bbox("dpz8zzzz").unwrap();
        let (xmin, ymin, xmax, ymax) = parse_polygon_bbox(&wkb, None);
        assert_eq!(xmin, expected.min().x);
        assert_eq!(ymin, expected.min().y);
        assert_eq!(xmax, expected.max().x);
        assert_eq!(ymax, expected.max().y);
    }

    #[test]
    fn test_geohashes_to_wkb_preserves_order() {
        let geohashes = ["dr5ru7", "dpz8zzzz", "9q8yy9ve"];
        let results = run(geohashes.to_vec(), None);
        assert_eq!(results.len(), geohashes.len());
        for (i, &hash) in geohashes.iter().enumerate() {
            let wkb = results[i].as_ref().unwrap();
            let expected = decode_bbox(hash).unwrap();
            let (xmin, ymin, xmax, ymax) = parse_polygon_bbox(wkb, None);
            assert_eq!(xmin, expected.min().x, "xmin mismatch at index {i} ({hash})");
            assert_eq!(ymin, expected.min().y, "ymin mismatch at index {i} ({hash})");
            assert_eq!(xmax, expected.max().x, "xmax mismatch at index {i} ({hash})");
            assert_eq!(ymax, expected.max().y, "ymax mismatch at index {i} ({hash})");
        }
    }

    #[test]
    fn test_geohashes_to_wkb_custom_thread_pool() {
        let results = run(vec!["dr5ru7", "dpz8zzzz"], Some(2));
        assert_eq!(results.len(), 2);
        for r in &results {
            assert_eq!(r.as_ref().unwrap().len(), 93);
        }
    }

    // ── serialize_bbox (EWKB) ────────────────────────────────────────────────

    #[test]
    fn test_bbox_to_ewkb_is_97_bytes() {
        assert_eq!(serialize_bbox(0.0, 0.0, 1.0, 1.0, Some(4326)).len(), 97);
    }

    #[test]
    fn test_bbox_to_ewkb_header() {
        parse_polygon_bbox(&serialize_bbox(0.0, 0.0, 1.0, 1.0, Some(4326)), Some(4326));
    }

    #[test]
    fn test_bbox_to_ewkb_coordinates() {
        let (xmin, ymin, xmax, ymax) = (-73.5853_f64, 45.5017, -73.5702, 45.5098);
        let (gx1, gy1, gx2, gy2) = parse_polygon_bbox(&serialize_bbox(xmin, ymin, xmax, ymax, Some(4326)), Some(4326));
        assert_eq!(gx1, xmin);
        assert_eq!(gy1, ymin);
        assert_eq!(gx2, xmax);
        assert_eq!(gy2, ymax);
    }

    #[test]
    fn test_bbox_to_ewkb_ring_is_closed() {
        let (xmin, ymin) = (-73.5853_f64, 45.5017_f64);
        let ewkb = serialize_bbox(xmin, ymin, -73.5702, 45.5098, Some(4326));
        assert_eq!(read_f64_le(&ewkb, 81), xmin); // point 4 x
        assert_eq!(read_f64_le(&ewkb, 89), ymin); // point 4 y
    }

    // ── geohashes_to_ewkb (pure Rust core) ───────────────────────────────────

    fn run_ewkb(
        geohashes: Vec<&str>,
        srid: u32,
        num_threads: Option<usize>,
    ) -> Vec<Result<Vec<u8>, GeohashError>> {
        let pool = num_threads.map(|n| rayon::ThreadPoolBuilder::new().num_threads(n).build().unwrap());
        geohashes_to_ewkb(geohashes.into_iter().map(String::from).collect(), srid, &pool)
    }

    #[test]
    fn test_geohashes_to_ewkb_empty() {
        assert!(run_ewkb(vec![], 4326, None).is_empty());
    }

    #[test]
    fn test_geohashes_to_ewkb_single() {
        let results = run_ewkb(vec!["dpz8zzzz"], 4326, None);
        assert_eq!(results.len(), 1);
        let ewkb = results.into_iter().next().unwrap().unwrap();
        let expected = decode_bbox("dpz8zzzz").unwrap();
        let (xmin, ymin, xmax, ymax) = parse_polygon_bbox(&ewkb, Some(4326));
        assert_eq!(xmin, expected.min().x);
        assert_eq!(ymin, expected.min().y);
        assert_eq!(xmax, expected.max().x);
        assert_eq!(ymax, expected.max().y);
    }

    #[test]
    fn test_geohashes_to_ewkb_preserves_order() {
        let geohashes = ["dr5ru7", "dpz8zzzz", "9q8yy9ve"];
        let results = run_ewkb(geohashes.to_vec(), 4326, None);
        assert_eq!(results.len(), geohashes.len());
        for (i, &hash) in geohashes.iter().enumerate() {
            let ewkb = results[i].as_ref().unwrap();
            let expected = decode_bbox(hash).unwrap();
            let (xmin, ymin, xmax, ymax) = parse_polygon_bbox(ewkb, Some(4326));
            assert_eq!(xmin, expected.min().x, "xmin mismatch at index {i} ({hash})");
            assert_eq!(ymin, expected.min().y, "ymin mismatch at index {i} ({hash})");
            assert_eq!(xmax, expected.max().x, "xmax mismatch at index {i} ({hash})");
            assert_eq!(ymax, expected.max().y, "ymax mismatch at index {i} ({hash})");
        }
    }

    #[test]
    fn test_geohashes_to_ewkb_custom_srid() {
        let results = run_ewkb(vec!["dr5ru7"], 32632, None);
        let ewkb = results.into_iter().next().unwrap().unwrap();
        assert_eq!(u32::from_le_bytes(ewkb[5..9].try_into().unwrap()), 32632u32);
    }

    #[test]
    fn test_geohashes_to_ewkb_invalid_geohash() {
        let results = run_ewkb(vec!["not-a-geohash!"], 4326, None);
        assert_eq!(results.len(), 1);
        assert!(results[0].is_err());
    }

    // ── ghbits (integer geohash representation) ──────────────────────────────

    /// Grid positions spanning every precision, both bit parities, and the grid
    /// corners where the interleaving is easiest to get wrong.
    fn ghbits_grid_samples() -> Vec<(u64, u64, usize)> {
        let mut out = Vec::new();
        for precision in 1..=ghbits::MAX_PRECISION {
            let (lon_bits, lat_bits) = ghbits::axis_bits(precision);
            let lon_max = (1u64 << lon_bits) - 1;
            let lat_max = (1u64 << lat_bits) - 1;
            for (i, j) in [
                (0, 0),
                (lon_max, lat_max),
                (0, lat_max),
                (lon_max, 0),
                (lon_max / 3, lat_max / 2),
                (1, lat_max - 1),
            ] {
                out.push((i, j, precision));
            }
        }
        out
    }

    /// ghbits must agree with the `geohash` crate it stands in for, in both
    /// directions: a cell's centre encodes to the cell's own hash, and that hash
    /// decodes to the cell's own bounding box.
    #[test]
    fn test_ghbits_matches_geohash_crate() {
        for (i, j, precision) in ghbits_grid_samples() {
            let packed = ghbits::merge(i, j, precision);
            assert_eq!(
                ghbits::split(packed, precision),
                (i, j),
                "split(merge(..)) must round-trip at p{precision}"
            );

            let bbox = ghbits::bbox(packed, precision);
            let hash = ghbits::unpack(packed, precision);
            assert_eq!(hash.len(), precision);

            let centre = (
                (bbox.min().x + bbox.max().x) / 2.0,
                (bbox.min().y + bbox.max().y) / 2.0,
            );
            assert_eq!(
                encode(centre.into(), precision).unwrap(),
                hash,
                "centre of cell ({i}, {j}) at p{precision} should encode to {hash}"
            );

            let want = decode_bbox(&hash).unwrap();
            for (label, w, g) in [
                ("xmin", want.min().x, bbox.min().x),
                ("ymin", want.min().y, bbox.min().y),
                ("xmax", want.max().x, bbox.max().x),
                ("ymax", want.max().y, bbox.max().y),
            ] {
                assert!((w - g).abs() < 1e-9, "{hash}: {label} {w} != {g}");
            }
        }
    }

    #[test]
    fn test_ghbits_pack_unpack_roundtrip() {
        for hash in [
            "d",
            "dr",
            "dr5",
            "dr5r",
            "dr5ru",
            "dr5ru7",
            "9q8yy9ve",
            "u4pruydqqvj",
            "zzzzzzzzzzzz",
            "000000000000",
        ] {
            let packed = ghbits::pack(hash).expect("valid geohash");
            assert_eq!(&ghbits::unpack(packed, hash.len()), hash);
        }
    }

    #[test]
    fn test_ghbits_pack_rejects_invalid_characters() {
        // 'a', 'i', 'l' and 'o' are excluded from the geohash alphabet.
        for bad in ["a", "dr5i", "dr5l", "dr5o", "dr5!", "dr5é"] {
            assert!(ghbits::pack(bad).is_none(), "{bad} should not pack");
        }
    }

    #[test]
    fn test_ghbits_pack_rejects_out_of_range_lengths() {
        assert!(ghbits::pack("").is_none(), "empty hash should not pack");
        // 13 characters need 65 bits, one more than the packed form holds.
        let too_long = "z".repeat(ghbits::MAX_PRECISION + 1);
        assert!(
            ghbits::pack(&too_long).is_none(),
            "{too_long} exceeds MAX_PRECISION and should not pack"
        );
    }

    #[test]
    fn test_ghbits_neighbors_match_geohash_crate() {
        // Away from the polar rows, ghbits and the geohash crate must agree.
        for hash in ["dr5ru7", "9q8yy9ve", "ezs42", "sp", "d", "u4pruydqqvj"] {
            let precision = hash.len();
            let r = neighbors(hash).unwrap();
            let want: HashSet<String> = [r.n, r.ne, r.e, r.se, r.s, r.sw, r.w, r.nw]
                .into_iter()
                .collect();

            let mut buf = [0u64; 8];
            let count = ghbits::neighbors(ghbits::pack(hash).unwrap(), precision, &mut buf);
            let got: HashSet<String> = buf[..count]
                .iter()
                .map(|&v| ghbits::unpack(v, precision))
                .collect();

            assert_eq!(want, got, "neighbours of {hash}");
        }
    }

    #[test]
    fn test_ghbits_neighbors_wrap_at_antimeridian() {
        // "b" is the north-west corner cell and "z" the north-east one; longitude
        // wraps, so they are neighbours across the antimeridian.
        let mut buf = [0u64; 8];
        let count = ghbits::neighbors(ghbits::pack("b").unwrap(), 1, &mut buf);
        let got: HashSet<String> = buf[..count]
            .iter()
            .map(|&v| ghbits::unpack(v, 1))
            .collect();
        assert!(got.contains("z"), "expected 'z' among {got:?}");
    }

    /// Latitude clamps rather than wrapping: there is no cell north of the top
    /// row. `geohash::neighbors` reports eight neighbours for "zzzzzzzzzzzz",
    /// three of which have wrapped over the pole to the far edge of the grid.
    #[test]
    fn test_ghbits_neighbors_clamp_at_poles() {
        for hash in ["zzzzzzzzzzzz", "000000000000", "bpbpbpbp"] {
            let precision = hash.len();
            let packed = ghbits::pack(hash).unwrap();
            let mut buf = [0u64; 8];
            let count = ghbits::neighbors(packed, precision, &mut buf);
            assert_eq!(count, 5, "{hash} sits in a polar row and has 5 neighbours");

            let (_, lat_i) = ghbits::split(packed, precision);
            for &neighbor in &buf[..count] {
                let (_, neighbor_lat) = ghbits::split(neighbor, precision);
                assert!(
                    neighbor_lat.abs_diff(lat_i) <= 1,
                    "{hash}: neighbour {} wrapped over the pole",
                    ghbits::unpack(neighbor, precision)
                );
            }
        }
    }

    #[test]
    fn test_ghbits_axis_bits_total_five_per_character() {
        for precision in 1..=ghbits::MAX_PRECISION {
            let (lon_bits, lat_bits) = ghbits::axis_bits(precision);
            assert_eq!(lon_bits + lat_bits, 5 * precision as u32);
            // Longitude takes the extra bit at odd totals; never more than one extra.
            assert!(lon_bits >= lat_bits && lon_bits - lat_bits <= 1);
        }
    }

    // ── polygons_to_geohashes ────────────────────────────────────────────────

    fn rect_polygon(xmin: f64, ymin: f64, xmax: f64, ymax: f64) -> Polygon {
        Polygon::new(
            geo_types::LineString::new(vec![
                geo_types::Coord { x: xmin, y: ymin },
                geo_types::Coord { x: xmax, y: ymin },
                geo_types::Coord { x: xmax, y: ymax },
                geo_types::Coord { x: xmin, y: ymax },
                geo_types::Coord { x: xmin, y: ymin },
            ]),
            vec![],
        )
    }

    /// A multipolygon must cover the union of its parts, even when one part's seed
    /// point falls inside a region already covered by an earlier part.
    ///
    /// `b`'s centroid (-73.555) lies inside `a` (-73.60 .. -73.55), so a walk that
    /// stops at cells an earlier part already accepted terminates on `b`'s seed and
    /// drops `b` entirely.
    #[test]
    fn test_multipolygon_part_with_seed_inside_earlier_part() {
        let a = rect_polygon(-73.60, 45.50, -73.55, 45.55);
        let b = rect_polygon(-73.590, 45.520, -73.520, 45.525);

        for fully_contained_only in [false, true] {
            let only_a = polygons_to_geohashes(vec![a.clone()], 7, fully_contained_only).unwrap();
            let only_b = polygons_to_geohashes(vec![b.clone()], 7, fully_contained_only).unwrap();
            let union: HashSet<String> = only_a.union(&only_b).cloned().collect();

            let combined =
                polygons_to_geohashes(vec![a.clone(), b.clone()], 7, fully_contained_only).unwrap();

            assert_eq!(
                combined,
                union,
                "fully_contained_only={fully_contained_only}: multipolygon dropped {} cells \
                 that the parts produce individually",
                union.difference(&combined).count()
            );
        }
    }

    /// Part order must not change the result.
    #[test]
    fn test_multipolygon_is_order_independent() {
        let a = rect_polygon(-73.60, 45.50, -73.55, 45.55);
        let b = rect_polygon(-73.590, 45.520, -73.520, 45.525);

        for fully_contained_only in [false, true] {
            let ab =
                polygons_to_geohashes(vec![a.clone(), b.clone()], 7, fully_contained_only).unwrap();
            let ba =
                polygons_to_geohashes(vec![b.clone(), a.clone()], 7, fully_contained_only).unwrap();
            assert_eq!(ab, ba, "fully_contained_only={fully_contained_only}");
        }
    }

    /// Geometry outside the geohash domain must error, not clamp: `cover_range`
    /// snaps indices onto the grid, so without the up-front check a polygon past
    /// the antimeridian covers nothing and one straddling it covers only the
    /// in-range half — both silently.
    #[test]
    fn test_out_of_range_polygons_are_rejected() {
        let beyond_antimeridian = rect_polygon(185.0, 10.0, 186.0, 11.0);
        let beyond_pole = rect_polygon(10.0, 90.5, 11.0, 91.0);
        let straddling = rect_polygon(179.5, 10.0, 180.5, 11.0);
        for polygon in [beyond_antimeridian, beyond_pole, straddling] {
            for fully_contained_only in [false, true] {
                assert!(
                    polygons_to_geohashes(vec![polygon.clone()], 6, fully_contained_only).is_err(),
                    "fully_contained_only={fully_contained_only}: expected an error for bbox {:?}",
                    polygon.bounding_rect().unwrap()
                );
            }
        }
        // The domain boundary itself is fine.
        let boundary = rect_polygon(179.0, 89.0, 180.0, 90.0);
        assert!(polygons_to_geohashes(vec![boundary], 4, false).is_ok());
    }

    /// An inner cover keeps cells that touch the polygon boundary without
    /// crossing it: containment is DE-9IM `contains` — interior inside, the
    /// boundaries allowed to meet — matching shapely and how holed polygons
    /// were always treated.
    ///
    /// This is a deliberate semantics change. The pre-descent fast path for
    /// hole-free polygons rejected boundary-touching cells, so a cell-aligned
    /// polygon like this one used to yield only the 12 interior children
    /// instead of all 32. It only shows on grid-aligned geometry; off the grid
    /// lines a boundary cell genuinely crosses the edge and is excluded either
    /// way.
    #[test]
    fn test_inner_cover_keeps_boundary_touching_cells() {
        let hash = "f2h30";
        let bbox = decode_bbox(hash).unwrap();
        let polygon = rect_polygon(bbox.min().x, bbox.min().y, bbox.max().x, bbox.max().y);

        let cover = polygons_to_geohashes(vec![polygon], 6, true).unwrap();

        let all_children: HashSet<String> = ghbits::BASE32
            .iter()
            .map(|&c| format!("{hash}{}", c as char))
            .collect();
        assert_eq!(cover, all_children);
    }

    // ── polygons_to_geohashes: hierarchical descent ──────────────────────────

    /// Independent oracle: test every cell in the bounding box directly, with no
    /// descent, no prepared geometry and no tree walk. Slow, so keep precision low.
    fn brute_force_cover(
        polygons: &[Polygon],
        precision: usize,
        fully_contained_only: bool,
    ) -> HashSet<String> {
        let mut out = HashSet::new();
        for polygon in polygons {
            let bbox = polygon.bounding_rect().unwrap();
            let (i0, i1, j0, j1) = cover_range(&bbox, precision);
            for i in i0..=i1 {
                for j in j0..=j1 {
                    let cell = ghbits::merge(i, j, precision);
                    let cell_poly = ghbits::bbox(cell, precision).to_polygon();
                    let hit = if fully_contained_only {
                        polygon.contains(&cell_poly)
                    } else {
                        polygon.intersects(&cell_poly)
                    };
                    if hit {
                        out.insert(ghbits::unpack(cell, precision));
                    }
                }
            }
        }
        out
    }

    fn load_fixture(wkt_str: &str) -> Vec<Polygon> {
        use wkt::TryFromWkt;
        let multi: geo::MultiPolygon<f64> = geo::MultiPolygon::try_from_wkt_str(wkt_str).unwrap();
        multi.0
    }

    #[test]
    fn test_matches_brute_force_on_fixtures() {
        // Precisions chosen so both flags yield a non-trivial cover while the
        // brute-force oracle stays cheap enough for a debug build.
        let fixtures = [
            (
                "verdun",
                include_str!("../tests/data/verdun_wkt.txt"),
                [6usize, 7],
            ),
            (
                "whitehorse",
                include_str!("../tests/data/whitehorse_wkt.txt"),
                [4, 5],
            ),
        ];
        for (name, wkt_str, precisions) in fixtures {
            let parts = load_fixture(wkt_str);
            for precision in precisions {
                for fully_contained_only in [false, true] {
                    let got = polygons_to_geohashes(parts.clone(), precision, fully_contained_only)
                        .unwrap();
                    let want = brute_force_cover(&parts, precision, fully_contained_only);
                    assert_eq!(
                        got,
                        want,
                        "{name} p{precision} fully_contained_only={fully_contained_only}: +{} -{}",
                        got.difference(&want).count(),
                        want.difference(&got).count()
                    );
                    assert!(
                        !want.is_empty(),
                        "{name} p{precision} fully_contained_only={fully_contained_only} is a \
                         vacuous case — pick a finer precision"
                    );
                }
            }
        }
    }

    #[test]
    fn test_precision_out_of_range_is_rejected() {
        let square = rect_polygon(-73.60, 45.50, -73.55, 45.55);
        for precision in [0usize, 13, 64] {
            assert!(
                polygons_to_geohashes(vec![square.clone()], precision, false).is_err(),
                "precision {precision} should be rejected"
            );
        }
    }

    /// A polygon flush against the geohash grid must still reach the cells it
    /// only touches along that grid line. Taking `floor` for the lower index
    /// would leave the west and south neighbours outside the seed range, and
    /// the descent only ever visits what it was seeded with — so `relate` would
    /// never get the chance to accept them.
    #[test]
    fn test_grid_aligned_polygon_reaches_touching_neighbours() {
        let precision = 5;
        let (lon_bits, lat_bits) = ghbits::axis_bits(precision);
        let lon_span = 360.0 / (1u64 << lon_bits) as f64;
        let lat_span = 180.0 / (1u64 << lat_bits) as f64;
        // Mid-latitude, so the cell has all eight neighbours and none of the
        // indices below clamp against an edge of the grid.
        let lon_i = ((-73.6 + 180.0) / lon_span).floor() as u64;
        let lat_j = ((45.5 + 90.0) / lat_span).floor() as u64;

        // The polygon *is* one cell, so its bounding box lies exactly on grid
        // lines on all four sides.
        let cell = ghbits::merge(lon_i, lat_j, precision);
        let polygon = ghbits::bbox(cell, precision).to_polygon();

        let got = polygons_to_geohashes(vec![polygon], precision, false).unwrap();

        let want: HashSet<String> = (lon_i - 1..=lon_i + 1)
            .flat_map(|i| {
                (lat_j - 1..=lat_j + 1)
                    .map(move |j| ghbits::unpack(ghbits::merge(i, j, precision), precision))
            })
            .collect();
        assert_eq!(
            got, want,
            "the cell and its eight touching neighbours should all be covered"
        );
    }

    /// The cover must refine consistently: every p6 cell's parent is a p5 cell.
    #[test]
    fn test_cover_refines_consistently() {
        let square = rect_polygon(-73.60, 45.50, -73.55, 45.55);
        let coarse = polygons_to_geohashes(vec![square.clone()], 5, false).unwrap();
        let fine = polygons_to_geohashes(vec![square], 6, false).unwrap();
        assert!(!fine.is_empty());
        for hash in &fine {
            assert!(
                coarse.contains(&hash[..5]),
                "p6 cell {hash} has no p5 parent in the coarse cover"
            );
        }
    }

    // ── expand_geohash_set ───────────────────────────────────────────────────

    fn hash_set(hashes: &[&str]) -> HashSet<String> {
        hashes.iter().map(|s| s.to_string()).collect()
    }

    /// Reference expansion built on the geohash crate, one ring at a time.
    fn reference_expand(input: &HashSet<String>, n_hops: usize) -> HashSet<String> {
        let mut all = input.clone();
        let mut frontier: Vec<String> = all.iter().cloned().collect();
        for _ in 0..n_hops {
            let mut next = Vec::new();
            for hash in &frontier {
                let r = neighbors(hash).unwrap();
                for n in [r.n, r.ne, r.e, r.se, r.s, r.sw, r.w, r.nw] {
                    if all.insert(n.clone()) {
                        next.push(n);
                    }
                }
            }
            frontier = next;
        }
        all
    }

    #[test]
    fn test_expand_matches_reference() {
        let input = hash_set(&["f2h30", "f2h31", "f2h32", "f2h33"]);
        for n_hops in [0usize, 1, 2, 5] {
            assert_eq!(
                expand_geohash_set(&input, n_hops).unwrap(),
                reference_expand(&input, n_hops),
                "n_hops={n_hops}"
            );
        }
    }

    #[test]
    fn test_expand_zero_hops_returns_input() {
        let input = hash_set(&["f2h30", "dr5ru"]);
        assert_eq!(expand_geohash_set(&input, 0).unwrap(), input);
    }

    #[test]
    fn test_expand_empty_set() {
        assert!(expand_geohash_set(&HashSet::new(), 3).unwrap().is_empty());
    }

    #[test]
    fn test_expand_grows_monotonically() {
        let input = hash_set(&["f2h30"]);
        let mut previous = expand_geohash_set(&input, 0).unwrap();
        for n_hops in 1..=4 {
            let current = expand_geohash_set(&input, n_hops).unwrap();
            assert!(
                previous.is_subset(&current),
                "hop {n_hops} lost cells from hop {}",
                n_hops - 1
            );
            assert!(current.len() > previous.len(), "hop {n_hops} added nothing");
            previous = current;
        }
    }

    /// A single cell expanded by one hop is its 3x3 block.
    #[test]
    fn test_expand_one_hop_is_the_surrounding_block() {
        let expanded = expand_geohash_set(&hash_set(&["f2h30"]), 1).unwrap();
        assert_eq!(expanded.len(), 9);
        assert!(expanded.contains("f2h30"));
    }

    #[test]
    fn test_expand_rejects_malformed_input() {
        // Character outside the geohash alphabet.
        assert!(expand_geohash_set(&hash_set(&["f2h3i"]), 1).is_err());
        // Validation happens even when no expansion is requested.
        assert!(expand_geohash_set(&hash_set(&["f2h3i"]), 0).is_err());
        // Mixed precisions cannot share one grid.
        assert!(expand_geohash_set(&hash_set(&["f2h30", "f2h3"]), 1).is_err());
        // Longer than the geohash crate accepts.
        assert!(expand_geohash_set(&hash_set(&["f2h30f2h30f2h"]), 1).is_err());
    }

    /// Expanding from a polar cell must stay in its own hemisphere.
    #[test]
    fn test_expand_at_the_pole_does_not_cross_over() {
        let expanded = expand_geohash_set(&hash_set(&["zzzz"]), 1).unwrap();
        assert_eq!(expanded.len(), 6, "one polar cell plus five neighbours");
        for hash in &expanded {
            let (_, lat_i) = ghbits::split(ghbits::pack(hash).unwrap(), 4);
            let (_, lat_bits) = ghbits::axis_bits(4);
            assert!(
                lat_i >= (1u64 << lat_bits) - 2,
                "{hash} jumped away from the north pole"
            );
        }
    }

    // ── n_hops_for ───────────────────────────────────────────────────────────

    #[test]
    fn test_n_hops_for_typical_cases() {
        // A p6 cell is roughly 1.2 km x 0.6 km, so 600 m is about one hop.
        assert_eq!(n_hops_for_core("f2h30f", 0.0).unwrap(), 0);
        assert!((1..=3).contains(&n_hops_for_core("f2h30f", 600.0).unwrap()));
        // Finer cells need proportionally more hops.
        assert!(
            n_hops_for_core("f2h30fg", 600.0).unwrap() > n_hops_for_core("f2h30f", 600.0).unwrap()
        );
    }

    #[test]
    fn test_n_hops_for_rejects_bad_expansion() {
        for bad in [f64::NAN, f64::INFINITY, -1.0] {
            assert!(
                n_hops_for_core("f2h30f", bad).is_err(),
                "{bad} should be rejected"
            );
        }
    }

    #[test]
    fn test_n_hops_for_rejects_invalid_geohash() {
        assert!(n_hops_for_core("not-a-geohash!", 100.0).is_err());
    }

    /// Near the poles a cell's width collapses with cos(latitude), so the hop
    /// count explodes. It must be refused rather than left to build a frontier
    /// of billions of cells.
    #[test]
    fn test_n_hops_for_refuses_runaway_polar_expansion() {
        // Top-row cells at fine precision: metres-per-cell tends to zero.
        for hash in ["zzzzzzzzz", "zzzzzzzzzzzz", "bpbpbpbpbpbp"] {
            let result = n_hops_for_core(hash, 1000.0);
            assert!(
                result.is_err(),
                "{hash} should refuse a 1 km expansion, got {result:?} hops"
            );
        }
    }

    /// The cap must not reject expansions that are merely large but workable.
    #[test]
    fn test_n_hops_for_allows_large_but_sane_expansion() {
        // 50 km at p6 near latitude 45: tens of hops.
        let hops = n_hops_for_core("f2h30f", 50_000.0).unwrap();
        assert!(hops > 10 && hops < MAX_EXPANSION_HOPS, "got {hops} hops");
    }

    /// The group hop count comes from the narrowest cell, not from whichever
    /// cell happens to be listed first — so it cannot depend on input order.
    #[test]
    fn test_group_hops_sized_on_the_narrowest_cell() {
        let equator = encode((10.0, 0.0).into(), 6).unwrap();
        let arctic = encode((10.0, 84.0).into(), 6).unwrap();

        let forward = n_hops_for_group_core([equator.as_str(), arctic.as_str()], 5000.0).unwrap();
        let backward = n_hops_for_group_core([arctic.as_str(), equator.as_str()], 5000.0).unwrap();
        assert_eq!(forward, backward);

        // The narrow arctic cell dictates the count for the whole group.
        assert_eq!(forward, n_hops_for_core(&arctic, 5000.0).unwrap());
        assert!(forward > n_hops_for_core(&equator, 5000.0).unwrap());
    }

    /// A runaway member must be refused wherever it sits in the group.
    #[test]
    fn test_group_hops_refuse_a_runaway_member_anywhere() {
        let sane = "f2h30fg2h";
        let polar = "zzzzzzzzz";
        assert!(n_hops_for_group_core([polar, sane], 1000.0).is_err());
        assert!(n_hops_for_group_core([sane, polar], 1000.0).is_err());
    }
}
