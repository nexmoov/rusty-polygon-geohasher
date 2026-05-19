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

def expand_geohashes(
    geohashes: list[str],
    expansion_m: float,
) -> list[str]: ...

def expand_geohash_mapping(
    groups: list[list[str]],
    expansion_m: float,
) -> list[list[str]]: ...
