
[![CI](https://github.com/nexmoov/rusty-polygon-geohasher/actions/workflows/CI.yml/badge.svg)](https://github.com/nexmoov/rusty-polygon-geohasher/actions/workflows/CI.yml)
[![Package version](https://img.shields.io/pypi/v/rusty-polygon-geohasher.svg)](https://pypi.org/project/rusty-polygon-geohasher)

# rusty-polygon-geohasher

A Rust-backed Python library for geohash operations: converting Shapely polygons to geohash sets,
encode/decode, and geography expansion. All compute-heavy paths use Rayon for parallelism.

Originally based on [polygon-geohasher](https://github.com/Bonsanto/polygon-geohasher) (pure Python).

The encode/decode functions are a maintained replacement for
[pygeohash-fast](https://github.com/PadenZach/pygeohash-fast), which is no longer actively maintained.


## Installing

```
pip install rusty-polygon-geohasher
```


## Usage

### Polygon → geohash set

```python
import geohash_polygon
from shapely import geometry

polygon = geometry.Polygon([(-99.1795917, 19.432134), (-99.1656847, 19.429034),
                            (-99.1776492, 19.414236), (-99.1795917, 19.432134)])

inner = geohash_polygon.polygon_to_geohashes(polygon, precision=7, inner=True)
outer = geohash_polygon.polygon_to_geohashes(polygon, precision=7, inner=False)
```

### Encode / decode

All functions use `(lng, lat)` order consistently — encode takes `(lng, lat)` and all decode
functions return `(lng, lat, ...)`.

```python
# Single encode/decode
h = geohash_polygon.encode(lng=-73.554, lat=45.508, precision=7)
lng, lat, lng_err, lat_err = geohash_polygon.decode_exactly(h)

# Batch (parallel via Rayon)
hashes = geohash_polygon.encode_many(lngs=[...], lats=[...], precision=7)
centers = geohash_polygon.decode_many(hashes)           # list of (lng, lat)
exact   = geohash_polygon.decode_many_exactly(hashes)   # list of (lng, lat, lng_err, lat_err)

# Optional thread count
geohash_polygon.encode_many(lngs, lats, 7, num_threads=4)
geohash_polygon.decode_many(hashes, num_threads=4)
geohash_polygon.decode_many_exactly(hashes, num_threads=4)
```

### Expand geohash mappings

Expand each geography's geohash set outward by a given distance in metres. Useful when you
want to count or join points-of-interest slightly outside a geography's boundary.

```python
# Single group
expanded = geohash_polygon.expand_geohashes(["f25dvz3", "f25dvz4", ...], expansion_m=500.0)

# Multiple groups — result[i] is the expanded version of groups[i]
groups = [["f25dvz3", "f25dvz4", ...], [...]]
expanded_groups = geohash_polygon.expand_geohash_mapping(groups, expansion_m=500.0)
```

The hop count is derived from the minimum cell dimension (accounting for latitude-dependent cell
width), so expansion is accurate in all directions including east/west at high latitudes. All
hashes in a group must have the same precision. Geography expansion runs in parallel across groups.

### WKB / EWKB output

Convert geohash bounding boxes to binary WKB or EWKB polygons for direct use
in DuckDB (`ST_GeomFromWKB`) or PostGIS geometry columns.

```python
hashes = ["f25dvz3", "f25dvz4", ...]

# Plain WKB — 93 bytes per hash
wkb_list = geohash_polygon.decode_many_to_wkb(hashes)

# EWKB with embedded SRID — 97 bytes per hash (default srid=4326)
ewkb_list = geohash_polygon.decode_many_to_ewkb(hashes)
ewkb_list = geohash_polygon.decode_many_to_ewkb(hashes, srid=32632)

# Optional thread count
geohash_polygon.decode_many_to_wkb(hashes, num_threads=4)
geohash_polygon.decode_many_to_ewkb(hashes, num_threads=4)
```
