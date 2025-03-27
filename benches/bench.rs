use criterion::{criterion_group, criterion_main, Criterion};
use geo::MultiPolygon;
use geohash_polygon::{polygons_to_geohashes, polygons_to_geohashes_handbrake};
use wkt::TryFromWkt;

fn bench_polygons_to_geohashes(c: &mut Criterion) {
    let verdun = include_str!("../tests/data/verdun_wkt.txt");
    let verdun: MultiPolygon<f64> = MultiPolygon::try_from_wkt_str(verdun).unwrap();

    let wh = include_str!("../tests/data/whitehorse_wkt.txt");
    let wh: MultiPolygon<f64> = MultiPolygon::try_from_wkt_str(wh).unwrap();
    c.bench_function("verdun 7 false", |b| {
        b.iter(|| polygons_to_geohashes(verdun.clone(), 7, false))
    });
    c.bench_function("verdun 7 true", |b| {
        b.iter(|| polygons_to_geohashes(verdun.clone(), 7, true))
    });
    c.bench_function("verdun 7 false oldfunc", |b| {
        b.iter(|| polygons_to_geohashes_handbrake(verdun.clone(), 7, false))
    });
    c.bench_function("verdun 7 true oldfunc", |b| {
        b.iter(|| polygons_to_geohashes_handbrake(verdun.clone(), 7, true))
    });

    c.bench_function("whitehorse 6 false oldfunc", |b| {
        b.iter(|| polygons_to_geohashes_handbrake(wh.clone(), 6, false))
    });
    c.bench_function("whitehorse 6 true oldfunc", |b| {
        b.iter(|| polygons_to_geohashes_handbrake(wh.clone(), 6, true))
    });
    c.bench_function("whitehorse 6 false", |b| {
        b.iter(|| polygons_to_geohashes(wh.clone(), 6, false))
    });
    c.bench_function("whitehorse 6 true", |b| {
        b.iter(|| polygons_to_geohashes(wh.clone(), 6, true))
    });
}

criterion_group!(benches, bench_polygons_to_geohashes);
criterion_main!(benches);
