use criterion::{criterion_group, criterion_main, Criterion};
use geo::MultiPolygon;
use geohash::decode_bbox;
use geohash_polygon::{expand_geohash_set, polygons_to_geohashes, polygons_to_geohashes_handbrake};
use std::collections::HashSet;
use wkt::TryFromWkt;

fn bench_polygons_to_geohashes(c: &mut Criterion) {
    let verdun = include_str!("../tests/data/verdun_wkt.txt");
    let verdun: MultiPolygon<f64> = MultiPolygon::try_from_wkt_str(verdun).unwrap();

    let wh = include_str!("../tests/data/whitehorse_wkt.txt");
    let wh: MultiPolygon<f64> = MultiPolygon::try_from_wkt_str(wh).unwrap();
    c.bench_function("verdun 7 false oldfunc", |b| {
        b.iter(|| polygons_to_geohashes_handbrake(verdun.clone(), 7, false))
    });
    c.bench_function("verdun 7 false", |b| {
        b.iter(|| polygons_to_geohashes(verdun.clone(), 7, false))
    });
    c.bench_function("verdun 7 true oldfunc", |b| {
        b.iter(|| polygons_to_geohashes_handbrake(verdun.clone(), 7, true))
    });
    c.bench_function("verdun 7 true", |b| {
        b.iter(|| polygons_to_geohashes(verdun.clone(), 7, true))
    });

    c.bench_function("whitehorse 6 false oldfunc", |b| {
        b.iter(|| polygons_to_geohashes_handbrake(wh.clone(), 6, false))
    });
    c.bench_function("whitehorse 6 false", |b| {
        b.iter(|| polygons_to_geohashes(wh.clone(), 6, false))
    });
    c.bench_function("whitehorse 6 true oldfunc", |b| {
        b.iter(|| polygons_to_geohashes_handbrake(wh.clone(), 6, true))
    });
    c.bench_function("whitehorse 6 true", |b| {
        b.iter(|| polygons_to_geohashes(wh.clone(), 6, true))
    });
}

fn bench_expand_geohash_set(c: &mut Criterion) {
    let wh = include_str!("../tests/data/whitehorse_wkt.txt");
    let wh: MultiPolygon<f64> = MultiPolygon::try_from_wkt_str(wh).unwrap();

    // Large high-latitude geography: good stress test for both hop count and
    // lat-dependent cell-width distortion.
    let geohashes: HashSet<String> = polygons_to_geohashes(wh, 6, false).unwrap();
    println!("Whitehorse p6 cell count: {}", geohashes.len());

    let sample_bbox = decode_bbox(geohashes.iter().next().unwrap()).unwrap();
    let cell_height_m = (sample_bbox.max().y - sample_bbox.min().y) * 111_000.0;

    for expansion_m in [500.0_f64, 2000.0_f64] {
        let n_hops = (expansion_m / cell_height_m).ceil() as usize;
        c.bench_function(
            &format!("expand_geohash_set whitehorse p6 {expansion_m}m ({n_hops} hops)"),
            |b| b.iter(|| expand_geohash_set(&geohashes, n_hops).unwrap()),
        );
    }
}

criterion_group!(benches, bench_polygons_to_geohashes, bench_expand_geohash_set);
criterion_main!(benches);
