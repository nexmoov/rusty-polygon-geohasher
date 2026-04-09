import shapely
import geohash_polygon
from polygon_geohasher.polygon_geohasher import (
    polygon_to_geohashes as polygon_to_geohashes_py,
)
import pytest


class _FakeGeo:
    """Object that has __geo_interface__ but returns a malformed mapping."""

    def __init__(self, geo_interface):
        self.__geo_interface__ = geo_interface


@pytest.mark.parametrize(
    "polygon, expected_message",
    [
        (
            _FakeGeo({"coordinates": []}),
            r"missing the required 'type' key",
        ),
        (
            _FakeGeo({"type": 42, "coordinates": []}),
            r"'type' value must be a string",
        ),
        (
            _FakeGeo({"type": "Polygon"}),
            r"missing the required 'coordinates' key",
        ),
        (
            _FakeGeo({"type": "Polygon", "coordinates": [[[0, 0], [1], [1, 1], [0, 0]]]}),
            r"invalid coordinate at index 1",
        ),
    ],
)
def test_exception_when_malformed_geo_interface(polygon, expected_message):
    with pytest.raises(ValueError, match=expected_message):
        geohash_polygon.polygon_to_geohashes(polygon, 3, True)


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
        r"Object does not implement __geo_interface__. Expected a Shapely Polygon or MultiPolygon.*",
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


@pytest.mark.parametrize(
    "level, inner",
    [
        (3, False),
        (3, True),
        (4, False),
        (4, True),
        (5, False),
        (5, True),
    ],
)
def test_crescent(level, inner, polygon_crescent):
    """Thin C-shaped polygon whose centroid is outside the polygon body.

    Before the interior_point() fallback was added, seed_interior_point_fast
    returned None and the caller fell back to the bbox center, which also lies
    outside the ring, causing the BFS to return an empty set.
    """
    result = geohash_polygon.polygon_to_geohashes(polygon_crescent, level, inner)
    reference = polygon_to_geohashes_py(polygon_crescent, level, inner)
    assert result == reference
    if not inner:
        # The crescent ring is real geometry — intersecting mode must find cells.
        assert len(result) > 0


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
def test_hole(level, inner, polygon_hole):
    assert geohash_polygon.polygon_to_geohashes(
        polygon_hole, level, inner
    ) == polygon_to_geohashes_py(polygon_hole, level, inner)
