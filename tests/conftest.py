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
