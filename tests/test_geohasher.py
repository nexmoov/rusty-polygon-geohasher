import shapely
import geohash_polygon
from polygon_geohasher.polygon_geohasher import (
    polygon_to_geohashes as polygon_to_geohashes_py,
)
import pytest


@pytest.mark.parametrize(
    "polygon, exception_message_idx",
    [
        (None, 0),
        (True, 0),
        ("string", 0),
        (1, 0),
        (1.0, 0),
        ([1, 2, 3], 0),
        ((1, 2, 3), 0),
        ({}, 0),
        (shapely.geometry.Point((-99.1795917, 19.432134)), 1),
    ],
)
def test_exception_when_invalid(polygon, exception_message_idx):
    exception_messages = [
        r"Exception while trying to extract Geometry. This function requires a Shapely Polygon or MultiPolygon.*",
        r"The geometry is not a Polygon or MultiPolygon",
    ]
    with pytest.raises(
        ValueError,
        match=exception_messages[exception_message_idx],
    ):
        geohash_polygon.polygon_to_geohashes(polygon, 3, True)


@pytest.mark.parametrize(
    "level, inner",
    [
        (1, False),
        (1, True),
        (2, False),
        (2, True),
        (3, False),
        (3, True),
        (4, False),
        (4, True),
        (5, False),
        (5, True),
        (6, False),
        (6, True),
        (7, False),
        (7, True),
    ],
)
def test_simple_polygon(level, inner):
    polygon = shapely.geometry.Polygon(
        [
            (-99.1795917, 19.432134),
            (-99.1656847, 19.429034),
            (-99.1776492, 19.414236),
            (-99.1795917, 19.432134),
        ]
    )
    assert geohash_polygon.polygon_to_geohashes(
        polygon, level, inner
    ) == polygon_to_geohashes_py(polygon, level, inner)


@pytest.mark.parametrize(
    "level, inner",
    [
        (1, False),
        (1, True),
        (2, False),
        (2, True),
        (3, False),
        (3, True),
        (4, False),
        (4, True),
        (5, False),
        (5, True),
        (6, False),
        (6, True),
        # (7, False),
        # (7, True),
    ],
)
def test_whitehorse(level, inner, polygon_whitehorse):
    assert geohash_polygon.polygon_to_geohashes(
        polygon_whitehorse, level, inner
    ) == polygon_to_geohashes_py(polygon_whitehorse, level, inner)


@pytest.mark.parametrize(
    "level, inner",
    [
        (1, False),
        (1, True),
        (2, False),
        (2, True),
        (3, False),
        (3, True),
        (4, False),
        (4, True),
        (5, False),
        (5, True),
        (6, False),
        (6, True),
        # (7, False),
        # (7, True),
    ],
)
def test_verdun(level, inner, polygon_verdun):
    assert geohash_polygon.polygon_to_geohashes(
        polygon_verdun, level, inner
    ) == polygon_to_geohashes_py(polygon_verdun, level, inner)
