import math

import pytest
import shapely
from shapely.geometry import shape


@pytest.fixture
def polygon_whitehorse():
    return shapely.from_wkt(open("tests/data/whitehorse_wkt.txt").read())


@pytest.fixture
def polygon_verdun():
    return shapely.from_wkt(open("tests/data/verdun_wkt.txt").read())


@pytest.fixture
def polygon_crescent():
    """Thin C-shaped ring whose centroid lies in the hollow interior.

    All fast interior probes (centroid, 24 offsets, bbox center, 4×4 grid)
    fall inside the hollow region and miss the thin ring body, which exposed
    the seed-point fallback bug where the BFS would start outside the polygon
    and return an empty result.
    """
    center_lon, center_lat = -73.5, 45.5
    outer_r, inner_r = 0.5, 0.49
    half_gap = math.radians(30)  # 60° gap opens to the right
    start, end = half_gap, 2 * math.pi - half_gap
    n = 64
    thetas = [start + (end - start) * i / (n - 1) for i in range(n)]
    outer = [(center_lon + outer_r * math.cos(t), center_lat + outer_r * math.sin(t)) for t in thetas]
    inner = [(center_lon + inner_r * math.cos(t), center_lat + inner_r * math.sin(t)) for t in reversed(thetas)]
    return shapely.geometry.Polygon(outer + inner + [outer[0]])


@pytest.fixture
def polygon_hole():
    return shape(
        {
            "type": "FeatureCollection",
            "name": "schefferville_demo_with_hole",
            "features": [
                {
                    "type": "Feature",
                    "properties": {
                        "name": "Schefferville (demo)",
                        "note": "Simplified test polygon with an inner ring (hole), WGS84 (EPSG:4326).",
                    },
                    "geometry": {
                        "type": "Polygon",
                        "coordinates": [
                            [
                                [-66.9000, 54.7800],
                                [-66.7500, 54.7800],
                                [-66.7500, 54.8600],
                                [-66.9000, 54.8600],
                                [-66.9000, 54.7800],
                            ],
                            [
                                [-66.8400, 54.8300],
                                [-66.8400, 54.8000],
                                [-66.8100, 54.8000],
                                [-66.8100, 54.8300],
                                [-66.8400, 54.8300],
                            ],
                        ],
                    },
                }
            ],
        }["features"][0]["geometry"]
    )
