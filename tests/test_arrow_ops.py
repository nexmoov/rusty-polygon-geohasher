"""Tests for expand_geohash_mapping_arrow. Skipped if pyarrow is not installed."""

import pytest
import geohash_polygon

pa = pytest.importorskip("pyarrow")


def _make_arrow_inputs(groups: list[tuple[str, list[str]]]):
    geog_ids = pa.array([g for g, _ in groups], type=pa.utf8())
    geohash_lists = pa.array([h for _, h in groups], type=pa.list_(pa.utf8()))
    return geog_ids, geohash_lists


def _result_to_pa(result) -> pa.RecordBatch:
    if isinstance(result, pa.RecordBatch):
        return result
    return pa.record_batch(result)


def test_expand_mapping_arrow_empty_input():
    geog_ids = pa.array([], type=pa.utf8())
    geohash_lists = pa.array([], type=pa.list_(pa.utf8()))
    result = _result_to_pa(
        geohash_polygon.expand_geohash_mapping_arrow(geog_ids, geohash_lists, 1.0)
    )
    assert len(result) == 0
    assert result.schema.names == ["geog_id", "geohash"]


def test_expand_mapping_arrow_single_cell_one_hop():
    center = geohash_polygon.encode(-73.5540, 45.5088, 7)
    geog_ids, geohash_lists = _make_arrow_inputs([("g1", [center])])
    result = _result_to_pa(
        geohash_polygon.expand_geohash_mapping_arrow(geog_ids, geohash_lists, 100.0)
    )
    assert len(result) == 9
    assert set(result.column("geog_id").to_pylist()) == {"g1"}
    assert center in result.column("geohash").to_pylist()


def test_expand_mapping_arrow_matches_list_version():
    coords = [(-73.5540, 45.5088), (-87.6298, 41.8781), (-79.3832, 43.6532)]
    geog_id_strs = ["g1", "g2", "g3"]
    groups_list = [[geohash_polygon.encode(lng, lat, 7)] for lng, lat in coords]
    expected = geohash_polygon.expand_geohash_mapping(groups_list, 100.0)

    geog_ids, geohash_lists = _make_arrow_inputs(list(zip(geog_id_strs, groups_list)))
    result = _result_to_pa(
        geohash_polygon.expand_geohash_mapping_arrow(geog_ids, geohash_lists, 100.0)
    )

    result_by_geog: dict[str, list[str]] = {gid: [] for gid in geog_id_strs}
    for gid, gh in zip(result.column("geog_id").to_pylist(), result.column("geohash").to_pylist()):
        result_by_geog[gid].append(gh)

    for i, gid in enumerate(geog_id_strs):
        assert set(result_by_geog[gid]) == set(expected[i])


def test_expand_mapping_arrow_negative_m_raises():
    center = geohash_polygon.encode(-73.5540, 45.5088, 7)
    geog_ids, geohash_lists = _make_arrow_inputs([("g1", [center])])
    with pytest.raises(ValueError, match="non-negative"):
        geohash_polygon.expand_geohash_mapping_arrow(geog_ids, geohash_lists, -1.0)


def test_expand_mapping_arrow_nan_m_raises():
    center = geohash_polygon.encode(-73.5540, 45.5088, 7)
    geog_ids, geohash_lists = _make_arrow_inputs([("g1", [center])])
    with pytest.raises(ValueError):
        geohash_polygon.expand_geohash_mapping_arrow(geog_ids, geohash_lists, float("nan"))


def test_expand_mapping_arrow_inf_m_raises():
    center = geohash_polygon.encode(-73.5540, 45.5088, 7)
    geog_ids, geohash_lists = _make_arrow_inputs([("g1", [center])])
    with pytest.raises(ValueError):
        geohash_polygon.expand_geohash_mapping_arrow(geog_ids, geohash_lists, float("inf"))


def test_expand_mapping_arrow_invalid_hash_raises():
    geog_ids, geohash_lists = _make_arrow_inputs([("g1", ["not_a_geohash!"])])
    with pytest.raises(Exception):
        geohash_polygon.expand_geohash_mapping_arrow(geog_ids, geohash_lists, 100.0)


def test_expand_mapping_arrow_empty_group_in_nonempty_input():
    center = geohash_polygon.encode(-73.5540, 45.5088, 7)
    geog_ids, geohash_lists = _make_arrow_inputs([("g1", []), ("g2", [center])])
    result = _result_to_pa(
        geohash_polygon.expand_geohash_mapping_arrow(geog_ids, geohash_lists, 100.0)
    )
    geog_id_col = result.column("geog_id").to_pylist()
    assert "g1" not in geog_id_col
    assert geog_id_col.count("g2") == 9


def test_expand_mapping_arrow_mixed_precision_in_group_raises():
    h5 = geohash_polygon.encode(-73.5540, 45.5088, 5)
    h7 = geohash_polygon.encode(-73.5540, 45.5088, 7)
    geog_ids, geohash_lists = _make_arrow_inputs([("g1", [h5, h7])])
    with pytest.raises(ValueError, match="same precision"):
        geohash_polygon.expand_geohash_mapping_arrow(geog_ids, geohash_lists, 100.0)


def test_expand_mapping_arrow_length_mismatch_raises():
    center = geohash_polygon.encode(-73.5540, 45.5088, 7)
    geog_ids = pa.array(["g1", "g2"], type=pa.utf8())
    geohash_lists = pa.array([[center]], type=pa.list_(pa.utf8()))
    with pytest.raises(ValueError, match="same length"):
        geohash_polygon.expand_geohash_mapping_arrow(geog_ids, geohash_lists, 100.0)


def test_expand_mapping_arrow_null_geog_id_raises():
    center = geohash_polygon.encode(-73.5540, 45.5088, 7)
    geog_ids = pa.array(["g1", None], type=pa.utf8())
    geohash_lists = pa.array([[center], [center]], type=pa.list_(pa.utf8()))
    with pytest.raises(ValueError, match="null"):
        geohash_polygon.expand_geohash_mapping_arrow(geog_ids, geohash_lists, 100.0)


def test_expand_mapping_arrow_multiple_geohashes_per_group():
    h1 = geohash_polygon.encode(-73.5540, 45.5088, 7)
    h2 = geohash_polygon.encode(-73.5530, 45.5088, 7)
    geog_ids, geohash_lists = _make_arrow_inputs([("g1", [h1, h2])])
    result = _result_to_pa(
        geohash_polygon.expand_geohash_mapping_arrow(geog_ids, geohash_lists, 100.0)
    )
    result_hashes = set(result.column("geohash").to_pylist())
    assert h1 in result_hashes
    assert h2 in result_hashes
    assert len(result_hashes) > 2
