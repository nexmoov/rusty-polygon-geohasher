# rusty-polygon-geohasher

Polygon Geohasher is an open source Python package for converting Shapely's polygons into a set of geohashes. It obtains the set of geohashes inside a polygon or geohashes that touch (intersect) the polygon. 

The library is based on the [polygon-geohasher](https://github.com/Bonsanto/polygon-geohasher) library which is implemented in pure python. The main difference is that this library is implemented in Rust, yielding a significant performance improvement.


## Installing
You can get the library from the Python Package Index (PyPI) using pip:

`$ pip install rusty-polygon-geohasher`


## Usage
Here are some simple examples:

```python
from polygon_geohasher.polygon_geohasher import polygon_to_geohashes 
from shapely import geometry

polygon = geometry.Polygon([(-99.1795917, 19.432134), (-99.1656847, 19.429034),
                            (-99.1776492, 19.414236), (-99.1795917, 19.432134)])
inner_geohashes_polygon = geohash_polygon.polygon_to_geohashes(polygon, 7, inner=True)
outer_geohashes_polygon = geohash_polygon.polygon_to_geohashes(polygon, 7, inner=False)
```
