import pytest
import shapely


@pytest.fixture
def polygon_whitehorse():
    return shapely.from_wkt(open("tests/data/whitehorse_wkt.txt").read())


@pytest.fixture
def polygon_verdun():
    return shapely.from_wkt(open("tests/data/verdun_wkt.txt").read())
