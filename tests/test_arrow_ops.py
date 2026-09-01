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


def test_arrow_null_geohash_reports_the_null():
    """A null inside a list used to surface as a precision mismatch."""
    geog_ids = pa.array(["a", "b"], type=pa.string())
    lists = pa.array([["f2h30", None], ["f2h31"]], type=pa.list_(pa.string()))
    with pytest.raises(ValueError, match=r"contains a null at position 1"):
        geohash_polygon.expand_geohash_mapping_arrow(geog_ids, lists, 100.0)


# ── decode_many_to_wkb_arrow / decode_many_to_ewkb_arrow ─────────────────────

HASHES = ["dr5ru7", "dpz8zzzz", "9q8yy9ve", "f2h30"]


def test_wkb_arrow_matches_the_list_api():
    result = pa.array(geohash_polygon.decode_many_to_wkb_arrow(pa.array(HASHES, type=pa.utf8())))
    assert result.type == pa.large_binary()
    assert result.to_pylist() == geohash_polygon.decode_many_to_wkb(HASHES)


def test_ewkb_arrow_matches_the_list_api():
    result = pa.array(
        geohash_polygon.decode_many_to_ewkb_arrow(pa.array(HASHES, type=pa.utf8()), 32632)
    )
    assert result.to_pylist() == geohash_polygon.decode_many_to_ewkb(HASHES, 32632)


def test_ewkb_arrow_srid_defaults_to_4326():
    array = pa.array(HASHES, type=pa.utf8())
    default = pa.array(geohash_polygon.decode_many_to_ewkb_arrow(array))
    explicit = pa.array(geohash_polygon.decode_many_to_ewkb_arrow(array, 4326))
    assert default.to_pylist() == explicit.to_pylist()


def test_wkb_arrow_row_widths():
    array = pa.array(HASHES, type=pa.utf8())
    wkb = pa.array(geohash_polygon.decode_many_to_wkb_arrow(array))
    ewkb = pa.array(geohash_polygon.decode_many_to_ewkb_arrow(array))
    assert all(len(v) == 93 for v in wkb.to_pylist())
    assert all(len(v) == 97 for v in ewkb.to_pylist())


def test_wkb_arrow_preserves_nulls():
    array = pa.array(["dr5ru7", None, "f2h30"], type=pa.utf8())
    result = pa.array(geohash_polygon.decode_many_to_wkb_arrow(array))
    assert result.null_count == 1
    assert result[1].as_py() is None
    assert result[0].as_py() == geohash_polygon.decode_many_to_wkb(["dr5ru7"])[0]
    assert result[2].as_py() == geohash_polygon.decode_many_to_wkb(["f2h30"])[0]


def test_wkb_arrow_accepts_large_utf8():
    small = pa.array(HASHES, type=pa.utf8())
    large = pa.array(HASHES, type=pa.large_utf8())
    assert (
        pa.array(geohash_polygon.decode_many_to_wkb_arrow(small)).to_pylist()
        == pa.array(geohash_polygon.decode_many_to_wkb_arrow(large)).to_pylist()
    )


def test_wkb_arrow_empty():
    result = pa.array(geohash_polygon.decode_many_to_wkb_arrow(pa.array([], type=pa.utf8())))
    assert len(result) == 0


def test_wkb_arrow_sliced_input():
    array = pa.array(HASHES, type=pa.utf8()).slice(1, 2)
    result = pa.array(geohash_polygon.decode_many_to_wkb_arrow(array))
    assert result.to_pylist() == geohash_polygon.decode_many_to_wkb(HASHES[1:3])


def test_wkb_arrow_rejects_non_string_array():
    with pytest.raises(ValueError, match="Utf8 or LargeUtf8"):
        geohash_polygon.decode_many_to_wkb_arrow(pa.array([1, 2, 3], type=pa.int64()))


def test_wkb_arrow_rejects_invalid_geohash():
    with pytest.raises(ValueError):
        geohash_polygon.decode_many_to_wkb_arrow(pa.array(["not-a-geohash!"], type=pa.utf8()))


def test_wkb_arrow_honours_num_threads():
    array = pa.array(HASHES, type=pa.utf8())
    single = pa.array(geohash_polygon.decode_many_to_wkb_arrow(array, num_threads=1))
    multi = pa.array(geohash_polygon.decode_many_to_wkb_arrow(array, num_threads=4))
    assert single.to_pylist() == multi.to_pylist()


def test_wkb_arrow_round_trips_a_larger_column():
    """Enough rows to cross rayon's chunk boundaries."""
    hashes = [geohash_polygon.encode(-73.5 + i * 1e-4, 45.5 + i * 1e-4, 7) for i in range(5000)]
    result = pa.array(geohash_polygon.decode_many_to_wkb_arrow(pa.array(hashes, type=pa.utf8())))
    assert result.to_pylist() == geohash_polygon.decode_many_to_wkb(hashes)
