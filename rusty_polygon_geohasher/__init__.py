from importlib.resources import files
import duckdb
import platform
import sys

def _platform_dir():
    m = platform.machine().lower()
    s = sys.platform
    if s.startswith("linux"):
        return "linux_aarch64" if "aarch64" in m or "arm64" in m else "linux_amd64"
    if s == "darwin":
        return "mac_universal2"    # ship a universal2 build
    if s.startswith("win"):
        return "win_arm64" if "arm64" in m else "win_amd64"
    raise RuntimeError(f"Unsupported platform: {s} {m}")

def load_duckdb_extension(conn=None):
    """Loads the bundled DuckDB extension and returns a DuckDB connection."""
    ext_path = files("rusty_polygon_geohasher") / "_duckdb" / _platform_dir() / "geohash.duckdb_extension"
    if conn is None:
        conn = duckdb.connect()
    conn.load_extension(str(ext_path))  # equivalent to SQL: LOAD 'path'
    return conn
