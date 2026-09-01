from typing import Any

def polygon_to_geohashes(
    py_polygon: Any,
    precision: int,
    inner: bool,
) -> set[str]: ...

def encode(lng: float, lat: float, precision: int) -> str: ...

def encode_many(
    lngs: list[float],
    lats: list[float],
    precision: int,
    num_threads: int | None = None,
) -> list[str]: ...

def decode_exactly(hash_str: str) -> tuple[float, float, float, float]: ...

def decode_many(
    geohashes: list[str],
    num_threads: int | None = None,
) -> list[tuple[float, float]]: ...

def decode_many_exactly(
    geohashes: list[str],
    num_threads: int | None = None,
) -> list[tuple[float, float, float, float]]: ...

def decode_many_to_wkb(
    geohashes: list[str],
    num_threads: int | None = None,
) -> list[bytes]: ...

def decode_many_to_ewkb(
    geohashes: list[str],
    srid: int = 4326,
    num_threads: int | None = None,
) -> list[bytes]: ...

def encode_many_arrow(
    lngs: Any,
    lats: Any,
    precision: int,
    num_threads: int | None = None,
) -> Any:
    """Encode coordinate Arrays to geohashes, via Arrow.

    lngs, lats: pyarrow Float64 Arrays of equal length (single chunk)
    Returns a LargeUtf8 Array; a row is null if either coordinate is null.
    Convert to pyarrow via pa.array(result).
    """
    ...

def decode_many_arrow(
    geohashes: Any,
    num_threads: int | None = None,
) -> Any:
    """Decode geohashes to cell centres, via Arrow.

    geohashes: pyarrow Utf8 or LargeUtf8 Array (single chunk)
    Returns a RecordBatch with schema (lng: Float64, lat: Float64).
    Convert to pyarrow via pa.record_batch(result).
    """
    ...

def decode_many_exactly_arrow(
    geohashes: Any,
    num_threads: int | None = None,
) -> Any:
    """Decode geohashes to centres and half-extents, via Arrow.

    Returns a RecordBatch with schema
    (lng, lat, lng_err, lat_err), all Float64.
    """
    ...

def decode_many_to_wkb_arrow(
    geohashes: Any,
    num_threads: int | None = None,
) -> Any:
    """Decode geohashes to WKB polygon bounding boxes, via Arrow.

    geohashes: pyarrow Utf8 or LargeUtf8 Array (single chunk — call
        .combine_chunks() first)
    Returns a LargeBinary Array; null inputs give null outputs.
    Convert to pyarrow via pa.array(result).
    """
    ...

def decode_many_to_ewkb_arrow(
    geohashes: Any,
    srid: int = 4326,
    num_threads: int | None = None,
) -> Any:
    """Decode geohashes to EWKB polygons with an embedded SRID, via Arrow.

    Like decode_many_to_wkb_arrow but with the SRID in the header, ready for a
    PostGIS geometry column without a separate ST_SetSRID.
    """
    ...

def expand_geohashes(
    geohashes: list[str],
    expansion_m: float,
) -> list[str]: ...

def expand_geohash_mapping(
    groups: list[list[str]],
    expansion_m: float,
) -> list[list[str]]: ...

def expand_geohash_mapping_arrow(
    geog_ids: Any,
    geohash_lists: Any,
    expansion_m: float,
    dictionary_geog_id: bool = False,
) -> Any:
    """Expand geohash groups using Arrow arrays for zero-copy I/O.

    geog_ids: pyarrow Utf8 Array (single chunk — call .combine_chunks() first)
    geohash_lists: pyarrow List<Utf8> Array (single chunk)
    dictionary_geog_id: emit geog_id as Dictionary<Int32, Utf8> rather than
        repeating each id once per row.
    Returns a RecordBatch with schema (geog_id, geohash: LargeUtf8), where
    geog_id is LargeUtf8 by default or Dictionary<Int32, Utf8> when
    dictionary_geog_id is set.
    Convert to pyarrow via pa.record_batch(result).
    """
    ...
