"""Tests for encode, decode_exactly, decode_many, encode_many, expand_geohash_mapping."""

import math
import struct
import pytest
import geohash_polygon


def haversine_m(lng1, lat1, lng2, lat2):
    R = 6_371_000
    phi1, phi2 = math.radians(lat1), math.radians(lat2)
    a = (math.sin(math.radians(lat2 - lat1) / 2) ** 2
         + math.cos(phi1) * math.cos(phi2) * math.sin(math.radians(lng2 - lng1) / 2) ** 2)
    return 2 * R * math.asin(math.sqrt(a))

# ── ported from pygeohash-fast ────────────
def test_encode_works():
    assert geohash_polygon.encode(-72.747917, 45.207615, 5) == "f2h30"

def test_decode_many_works():
    assert geohash_polygon.decode_many(["f2h30", "f2h30"]) == [(-72.75146484375, 45.19775390625), (-72.75146484375, 45.19775390625)]

def test_encode_many_works():
    lats = [47.1, 35.204]
    lngs = [-76.6, -80.8501]
    expected = ["f23e", "dnq8"]
    assert geohash_polygon.encode_many(lngs, lats, 4) == expected

# ── encode / decode_exactly round-trip ───────────────────────────────────────

def test_encode_returns_correct_length():
    for precision in range(1, 9):
        result = geohash_polygon.encode(-73.5540, 45.5088, precision)
        assert isinstance(result, str)
        assert len(result) == precision


def test_encode_decode_exactly_roundtrip():
    lat, lng, precision = 45.5088, -73.5540, 7
    encoded = geohash_polygon.encode(lng, lat, precision)
    decoded_lng, decoded_lat, lng_err, lat_err = geohash_polygon.decode_exactly(encoded)
    assert abs(decoded_lat - lat) <= lat_err
    assert abs(decoded_lng - lng) <= lng_err


def test_decode_exactly_error_bounds_nonzero():
    encoded = geohash_polygon.encode(-73.5540, 45.5088, 7)
    _, _, lat_err, lng_err = geohash_polygon.decode_exactly(encoded)
    assert lat_err > 0
    assert lng_err > 0


def test_decode_exactly_invalid_raises():
    with pytest.raises(ValueError):
        geohash_polygon.decode_exactly("not_a_geohash!")


# ── encode_many ───────────────────────────────────────────────────────────────

def test_encode_many_matches_encode():
    coords = [
        (-73.5540, 45.5088),
        (-79.3832, 43.6532),
        (-87.6298, 41.8781),
    ]
    lngs = [c[0] for c in coords]
    lats = [c[1] for c in coords]
    results = geohash_polygon.encode_many(lngs, lats, 7)
    for (lng, lat), result in zip(coords, results):
        assert result == geohash_polygon.encode(lng, lat, 7)


def test_encode_many_mismatched_lengths_raises():
    with pytest.raises(ValueError, match="same length"):
        geohash_polygon.encode_many([-73.0, -74.0], [45.0], 7)


def test_encode_many_invalid_precision_raises():
    with pytest.raises(Exception):
        geohash_polygon.encode_many([-73.0], [45.0], 0)


def test_encode_many_with_explicit_threads():
    lngs = [-73.5540, -79.3832]
    lats = [45.5088, 43.6532]
    result_single = geohash_polygon.encode_many(lngs, lats, 7, num_threads=1)
    result_multi = geohash_polygon.encode_many(lngs, lats, 7, num_threads=2)
    assert result_single == result_multi


# ── decode_many ───────────────────────────────────────────────────────────────

def test_decode_many_invalid_raises():
    with pytest.raises(Exception):
        geohash_polygon.decode_many(["not_a_geohash!"])


def test_decode_many_matches_decode_exactly():
    hashes = [
        geohash_polygon.encode(lng, lat, 7)
        for lng, lat in [(-73.5540, 45.5088), (-79.3832, 43.6532)]
    ]
    pairs = geohash_polygon.decode_many(hashes)
    assert len(pairs) == len(hashes)
    for (lat, lng), h in zip(pairs, hashes):
        expected_lat, expected_lng, _, _ = geohash_polygon.decode_exactly(h)
        assert abs(lat - expected_lat) < 1e-10
        assert abs(lng - expected_lng) < 1e-10


# ── decode_many_exactly ───────────────────────────────────────────────────────

def test_decode_many_exactly_invalid_raises():
    with pytest.raises(Exception):
        geohash_polygon.decode_many_exactly(["not_a_geohash!"])


def test_decode_many_exactly_matches_decode_exactly():
    hashes = [
        geohash_polygon.encode(lng, lat, 7)
        for lng, lat in [(-73.5540, 45.5088), (-79.3832, 43.6532)]
    ]
    results = geohash_polygon.decode_many_exactly(hashes)
    assert len(results) == len(hashes)
    for (lat, lng, lat_err, lng_err), h in zip(results, hashes):
        e_lat, e_lng, e_lat_err, e_lng_err = geohash_polygon.decode_exactly(h)
        assert abs(lat - e_lat) < 1e-10
        assert abs(lng - e_lng) < 1e-10
        assert abs(lat_err - e_lat_err) < 1e-10
        assert abs(lng_err - e_lng_err) < 1e-10


def test_decode_many_exactly_with_explicit_threads():
    hashes = [geohash_polygon.encode(-73.5540, 45.5088, 7)]
    r1 = geohash_polygon.decode_many_exactly(hashes, num_threads=1)
    r2 = geohash_polygon.decode_many_exactly(hashes, num_threads=2)
    assert r1 == r2


# ── expand_geohashes (single group) ──────────────────────────────────────────

def test_expand_geohashes_negative_m_raises():
    center = geohash_polygon.encode(-73.5540, 45.5088, 7)
    with pytest.raises(ValueError, match="non-negative"):
        geohash_polygon.expand_geohashes([center], -1.0)


def test_expand_geohashes_nan_m_raises():
    center = geohash_polygon.encode(-73.5540, 45.5088, 7)
    with pytest.raises(ValueError):
        geohash_polygon.expand_geohashes([center], float("nan"))


def test_expand_geohashes_inf_m_raises():
    center = geohash_polygon.encode(-73.5540, 45.5088, 7)
    with pytest.raises(ValueError):
        geohash_polygon.expand_geohashes([center], float("inf"))


def test_expand_geohashes_invalid_hash_raises():
    with pytest.raises(Exception):
        geohash_polygon.expand_geohashes(["not_a_geohash!"], 0.10)


def test_expand_mapping_invalid_hash_raises():
    with pytest.raises(Exception):
        geohash_polygon.expand_geohash_mapping([["not_a_geohash!"]], 0.10)


def test_expand_geohashes_mixed_precision_raises():
    h5 = geohash_polygon.encode(-73.5540, 45.5088, 5)
    h7 = geohash_polygon.encode(-73.5540, 45.5088, 7)
    with pytest.raises(ValueError, match="same precision"):
        geohash_polygon.expand_geohashes([h5, h7], 100.0)


def test_expand_mapping_mixed_precision_raises():
    h5 = geohash_polygon.encode(-73.5540, 45.5088, 5)
    h7 = geohash_polygon.encode(-73.5540, 45.5088, 7)
    with pytest.raises(ValueError, match="same precision"):
        geohash_polygon.expand_geohash_mapping([[h5, h7]], 100.0)


def test_expand_geohashes_empty():
    assert geohash_polygon.expand_geohashes([], 1.0) == []


def test_expand_geohashes_zero_hops_returns_original():
    center = geohash_polygon.encode(-73.5540, 45.5088, 7)
    result = geohash_polygon.expand_geohashes([center], 0.0)
    assert result == [center]


def test_expand_geohashes_one_hop_gives_nine_cells():
    # One geohash expanded by 1 hop: original + 8 neighbors = 9 cells.
    # n_hops = ceil(100 / ~152) = 1
    center = geohash_polygon.encode(-73.5540, 45.5088, 7)
    result = geohash_polygon.expand_geohashes([center], 100.0)
    assert len(result) == 9
    assert center in result


def test_expand_geohashes_preserves_input():
    hashes = [geohash_polygon.encode(-73.5540 + i * 0.002, 45.5088, 7) for i in range(3)]
    result = geohash_polygon.expand_geohashes(hashes, 100.0)
    assert set(hashes).issubset(set(result))
    assert len(result) > len(hashes)


def test_expand_geohashes_count_grows_with_expansion_m():
    center = geohash_polygon.encode(-73.5540, 45.5088, 7)
    hashes_1hop = geohash_polygon.expand_geohashes([center], 100.0)
    hashes_2hop = geohash_polygon.expand_geohashes([center], 300.0)
    assert len(hashes_2hop) > len(hashes_1hop)


def _expansion_max_dist(orig_lng, orig_lat, orig_lng_err, orig_lat_err, expansion_m):
    """Upper bound on center-to-center distance for a BFS expansion.

    BFS uses n_hops = ceil(expansion_m / min_full_dim). Worst case is always
    stepping diagonally, so max distance = n_hops × full_cell_diagonal.
    """
    half_h = orig_lat_err * 111_000
    half_w = orig_lng_err * 111_320 * math.cos(math.radians(orig_lat))
    n_hops = math.ceil(expansion_m / (2 * min(half_h, half_w)))
    # 1% buffer for flat-Earth vs haversine divergence
    return n_hops * math.sqrt((2 * half_h) ** 2 + (2 * half_w) ** 2) * 1.01


def test_expand_geohashes_added_cells_within_distance():
    # Every added cell center must be within the BFS max-distance bound.
    expansion_m = 300.0
    center = geohash_polygon.encode(-73.5540, 45.5088, 7)
    orig_lng, orig_lat = geohash_polygon.decode_many([center])[0]
    _, _, orig_lng_err, orig_lat_err = geohash_polygon.decode_exactly(center)
    max_dist = _expansion_max_dist(orig_lng, orig_lat, orig_lng_err, orig_lat_err, expansion_m)
    expanded = set(geohash_polygon.expand_geohashes([center], expansion_m))
    for h in expanded - {center}:
        lng, lat = geohash_polygon.decode_many([h])[0]
        dist = haversine_m(orig_lng, orig_lat, lng, lat)
        assert dist <= max_dist, f"{h} center is {dist:.1f}m from origin, max allowed {max_dist:.1f}m"


def test_expand_geohashes_ew_coverage_at_high_latitude():
    # At ~60°N (Whitehorse), cell width ≈ half cell height. Without the lat-adjusted
    # hop count, east/west expansion uses too few hops. Verify all added cells stay
    # within the BFS max-distance bound (which would be violated pre-fix for diagonal cells).
    expansion_m = 500.0
    center = geohash_polygon.encode(-135.0, 60.7, 7)  # Whitehorse area
    orig_lng, orig_lat = geohash_polygon.decode_many([center])[0]
    _, _, orig_lng_err, orig_lat_err = geohash_polygon.decode_exactly(center)
    max_dist = _expansion_max_dist(orig_lng, orig_lat, orig_lng_err, orig_lat_err, expansion_m)
    expanded = set(geohash_polygon.expand_geohashes([center], expansion_m))
    for h in expanded - {center}:
        lng, lat = geohash_polygon.decode_many([h])[0]
        dist = haversine_m(orig_lng, orig_lat, lng, lat)
        assert dist <= max_dist, f"{h} center is {dist:.1f}m from origin, max allowed {max_dist:.1f}m"


# ── expand_geohash_mapping (multiple groups) ──────────────────────────────────

def test_expand_mapping_empty_input():
    assert geohash_polygon.expand_geohash_mapping([], 1.0) == []


def test_expand_mapping_zero_hops_returns_original():
    center = geohash_polygon.encode(-73.5540, 45.5088, 7)
    result = geohash_polygon.expand_geohash_mapping([[center]], 0.0)
    assert len(result) == 1
    assert result[0] == [center]


def test_expand_mapping_single_cell_one_hop_gives_nine_cells():
    center = geohash_polygon.encode(-73.5540, 45.5088, 7)
    result = geohash_polygon.expand_geohash_mapping([[center]], 100.0)
    assert len(result) == 1
    assert len(result[0]) == 9
    assert center in result[0]


def test_expand_mapping_order_preserved():
    # Output[i] must correspond to input[i], not some arbitrary reordering.
    coords = [(-73.5540, 45.5088), (-87.6298, 41.8781), (-79.3832, 43.6532)]
    groups = [[geohash_polygon.encode(lng, lat, 7)] for lng, lat in coords]
    result = geohash_polygon.expand_geohash_mapping(groups, 100.0)
    assert len(result) == 3
    for i, (lng, lat) in enumerate(coords):
        expected_center = geohash_polygon.encode(lng, lat, 7)
        assert expected_center in result[i]


def test_expand_mapping_mixed_precisions():
    # Groups at different precision levels each use their own cell size for n_hops.
    # p5 cells are ~4900 m; p7 cells are ~152 m.
    # With a 1000 m expansion:
    #   p5: ceil(1000 / 4900) = 1 hop  → 9 cells
    #   p7: ceil(1000 / 152)  = 7 hops → many more cells
    # If a global sample from p5 were used, p7 would wrongly get 1 hop too.
    h5 = geohash_polygon.encode(-73.5540, 45.5088, 5)
    h7 = geohash_polygon.encode(-87.6298, 41.8781, 7)
    result = geohash_polygon.expand_geohash_mapping([[h5], [h7]], 1000.0)
    assert len(result[0]) == 9      # p5: 1 hop
    assert len(result[1]) > 9       # p7: many more hops


def test_expand_mapping_two_groups_are_independent():
    # Two groups far apart should expand independently with no crosstalk.
    h1 = geohash_polygon.encode(-73.5540, 45.5088, 7)  # Montreal
    h2 = geohash_polygon.encode(-87.6298, 41.8781, 7)  # Chicago
    result = geohash_polygon.expand_geohash_mapping([[h1], [h2]], 100.0)
    assert len(result[0]) == 9
    assert len(result[1]) == 9
    assert set(result[0]).isdisjoint(set(result[1]))


# ── decode_many_to_wkb / decode_many_to_ewkb ─────────────────────────────────

def test_decode_many_to_wkb_returns_bytes():
    h = geohash_polygon.encode(-73.554, 45.508, 7)
    results = geohash_polygon.decode_many_to_wkb([h])
    assert len(results) == 1
    assert isinstance(results[0], bytes)
    assert len(results[0]) == 93


def test_decode_many_to_wkb_invalid_raises():
    with pytest.raises(ValueError):
        geohash_polygon.decode_many_to_wkb(["not_a_geohash!"])


def test_decode_many_to_wkb_order_preserved():
    hashes = [geohash_polygon.encode(-73.554 + i * 0.002, 45.508, 7) for i in range(3)]
    results = geohash_polygon.decode_many_to_wkb(hashes)
    for h, wkb in zip(hashes, results):
        lng, lat, lng_err, lat_err = geohash_polygon.decode_exactly(h)
        xmin = struct.unpack_from("<d", wkb, 13)[0]
        ymin = struct.unpack_from("<d", wkb, 21)[0]
        xmax = struct.unpack_from("<d", wkb, 29)[0]
        ymax = struct.unpack_from("<d", wkb, 53)[0]
        assert abs(xmin - (lng - lng_err)) < 1e-10, f"xmin mismatch for {h}"
        assert abs(ymin - (lat - lat_err)) < 1e-10, f"ymin mismatch for {h}"
        assert abs(xmax - (lng + lng_err)) < 1e-10, f"xmax mismatch for {h}"
        assert abs(ymax - (lat + lat_err)) < 1e-10, f"ymax mismatch for {h}"


def test_decode_many_to_wkb_with_explicit_threads():
    hashes = [geohash_polygon.encode(-73.554 + i * 0.001, 45.508 + i * 0.001, 7) for i in range(10)]
    r1 = geohash_polygon.decode_many_to_wkb(hashes, num_threads=1)
    r4 = geohash_polygon.decode_many_to_wkb(hashes, num_threads=4)
    assert r1 == r4


def test_decode_many_to_ewkb_returns_bytes():
    h = geohash_polygon.encode(-73.554, 45.508, 7)
    results = geohash_polygon.decode_many_to_ewkb([h])
    assert len(results) == 1
    assert isinstance(results[0], bytes)
    assert len(results[0]) == 97


def test_decode_many_to_ewkb_srid_default_4326():
    h = geohash_polygon.encode(-73.554, 45.508, 7)
    ewkb = geohash_polygon.decode_many_to_ewkb([h])[0]
    srid = struct.unpack_from("<I", ewkb, 5)[0]
    assert srid == 4326


def test_decode_many_to_ewkb_custom_srid():
    h = geohash_polygon.encode(-73.554, 45.508, 7)
    ewkb = geohash_polygon.decode_many_to_ewkb([h], srid=32632)[0]
    srid = struct.unpack_from("<I", ewkb, 5)[0]
    assert srid == 32632


def test_decode_many_to_ewkb_invalid_raises():
    with pytest.raises(ValueError):
        geohash_polygon.decode_many_to_ewkb(["not_a_geohash!"])


def test_decode_many_to_wkb_roundtrip():
    from shapely.wkb import loads
    h = geohash_polygon.encode(-73.554, 45.508, 7)
    wkb = geohash_polygon.decode_many_to_wkb([h])[0]
    polygon = loads(wkb)
    result = geohash_polygon.polygon_to_geohashes(polygon, precision=7, inner=False)
    assert h in result


@pytest.mark.parametrize("precision", [5, 6, 7, 8])
def test_decode_many_to_wkb_roundtrip_precision(precision):
    from shapely.wkb import loads
    h = geohash_polygon.encode(-73.554, 45.508, precision)
    wkb = geohash_polygon.decode_many_to_wkb([h])[0]
    polygon = loads(wkb)
    result = geohash_polygon.polygon_to_geohashes(polygon, precision=precision, inner=False)
    assert h in result


def test_decode_many_to_wkb_roundtrip_cluster():
    # A single-cell round-trip only exercises a convex rectangle. Unioning 9
    # adjacent cells produces a non-trivial polygon where interior-point
    # detection and BFS seeding are exercised along shared edges — a failure
    # mode that a lone bbox test would miss entirely.
    from shapely.wkb import loads
    from shapely.ops import unary_union
    center = geohash_polygon.encode(-73.554, 45.508, 6)
    cluster = geohash_polygon.expand_geohashes([center], 100.0)  # 1 hop → 9 cells
    wkb_list = geohash_polygon.decode_many_to_wkb(cluster)
    union = unary_union([loads(w) for w in wkb_list])
    result = set(geohash_polygon.polygon_to_geohashes(union, precision=6, inner=False))
    assert set(cluster).issubset(result)
