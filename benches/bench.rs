use criterion::{criterion_group, criterion_main, BatchSize, Criterion};
use geo::MultiPolygon;
use geohash::decode_bbox;
use geohash_polygon::{expand_geohash_set, geohashes_to_wkb, polygons_to_geohashes};
use std::collections::HashSet;
use std::time::Duration;
use wkt::TryFromWkt;

fn verdun() -> MultiPolygon<f64> {
    MultiPolygon::try_from_wkt_str(include_str!("../tests/data/verdun_wkt.txt")).unwrap()
}

fn whitehorse() -> MultiPolygon<f64> {
    MultiPolygon::try_from_wkt_str(include_str!("../tests/data/whitehorse_wkt.txt")).unwrap()
}

fn bench_polygons_to_geohashes(c: &mut Criterion) {
    let verdun = verdun();
    let wh = whitehorse();

    let mut group = c.benchmark_group("polygons_to_geohashes");
    // The cover grows 32x per precision level, so the coarse and fine ends need
    // very different sample counts to finish in a sane time.
    //
    // `polygons_to_geohashes` consumes its input, so each iteration needs its
    // own copy — cloned in setup, like `bench_geohashes_to_wkb`, to keep the
    // MultiPolygon clone out of the measurement.
    for (name, polygon, precision) in [
        ("verdun", &verdun, 7usize),
        ("verdun", &verdun, 8),
        ("whitehorse", &wh, 6),
    ] {
        for inner in [false, true] {
            group.bench_function(format!("{name} p{precision} inner={inner}"), |b| {
                b.iter_batched(
                    || polygon.clone(),
                    |input| polygons_to_geohashes(input, precision, inner),
                    BatchSize::LargeInput,
                )
            });
        }
    }
    group.finish();

    // Above PARALLEL_COVER_MIN_CELLS the cover fans out across Rayon workers.
    // These are the arms that show it.
    let mut group = c.benchmark_group("polygons_to_geohashes/large");
    group
        .sample_size(10)
        .measurement_time(Duration::from_secs(10));
    for (name, polygon, precision) in [("verdun", &verdun, 9usize), ("whitehorse", &wh, 7)] {
        for inner in [false, true] {
            group.bench_function(format!("{name} p{precision} inner={inner}"), |b| {
                b.iter_batched(
                    || polygon.clone(),
                    |input| polygons_to_geohashes(input, precision, inner),
                    BatchSize::LargeInput,
                )
            });
        }
    }
    group.finish();
}

fn bench_expand_geohash_set(c: &mut Criterion) {
    // Large high-latitude geography: good stress test for both hop count and
    // lat-dependent cell-width distortion.
    let geohashes: HashSet<String> = polygons_to_geohashes(whitehorse(), 6, false).unwrap();
    println!("Whitehorse p6 cell count: {}", geohashes.len());

    let sample_bbox = decode_bbox(geohashes.iter().next().unwrap()).unwrap();
    let cell_height_m = (sample_bbox.max().y - sample_bbox.min().y) * 111_000.0;

    let mut group = c.benchmark_group("expand_geohash_set");
    // 0 hops is the "expansion_m = 0" path, which used to pay a full boundary scan.
    for expansion_m in [0.0_f64, 500.0, 2000.0] {
        let n_hops = (expansion_m / cell_height_m).ceil() as usize;
        group.bench_function(
            format!("whitehorse p6 {expansion_m}m ({n_hops} hops)"),
            |b| b.iter(|| expand_geohash_set(&geohashes, n_hops).unwrap()),
        );
    }
    group.finish();
}

fn bench_geohashes_to_wkb(c: &mut Criterion) {
    // The Rust core of decode_many_to_wkb, without the Python boundary that
    // dominates the list-returning binding.
    let geohashes: Vec<String> = polygons_to_geohashes(whitehorse(), 6, false)
        .unwrap()
        .into_iter()
        .collect();

    let mut group = c.benchmark_group("geohashes_to_wkb");
    // `geohashes_to_wkb` takes the Vec by value, so each iteration needs its own
    // copy. Cloning inside `iter` would time one String allocation per hash on
    // top of the decode, which is most of what this benchmark reports otherwise.
    group.bench_function(format!("whitehorse p6 ({} hashes)", geohashes.len()), |b| {
        b.iter_batched(
            || geohashes.clone(),
            |input| geohashes_to_wkb(input, &None),
            BatchSize::LargeInput,
        )
    });
    group.finish();
}

criterion_group!(
    benches,
    bench_polygons_to_geohashes,
    bench_expand_geohash_set,
    bench_geohashes_to_wkb
);
criterion_main!(benches);
