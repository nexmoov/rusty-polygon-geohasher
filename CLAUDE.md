# rusty-polygon-geohasher

Rust library with Python bindings via PyO3/maturin. Changes to Rust code must be compiled before Python tests reflect them.

## Build & test

```
make test          # cargo test + maturin develop -r + pytest
make format        # cargo fmt + ruff
make check-rust    # clippy + fmt check
```

Or manually:
```
cargo test                         # Rust unit tests only
poetry run maturin develop -r      # compile Rust → Python extension
poetry run pytest tests            # Python tests
```

## Structure

- `src/lib.rs` — all Rust source and PyO3 bindings
- `geohash_polygon/__init__.pyi` — type stub for the Python package; **must be kept in sync with `src/lib.rs`** when adding, removing, or changing any `#[pyfunction]` signature
- `geohash_polygon/__init__.py` — package init that re-exports from the compiled extension
- `tests/` — Python tests (require compiled extension)
- `benches/` — criterion benchmarks
