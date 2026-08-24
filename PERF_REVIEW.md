# Code review: bugs & performance — checklist

Findings from a review of `src/lib.rs` (v0.6.4, commit `c4a9e30`).
All measurements taken on `tests/data/{verdun,whitehorse}_wkt.txt`, release build, Apple Silicon.
Prototypes that produced the numbers live in the session scratchpad under `perf/`.

Legend: **[H]** high / **[M]** medium / **[L]** low.

---

## Bugs

### [ ] B1 — MultiPolygon parts silently dropped **[H]**

`src/lib.rs:115-119`

`accepted_geohashes` persists across parts of a MultiPolygon (deliberately), but the early
`continue` skips **neighbour expansion**, not just re-testing. If part B's seed cell was
already accepted while processing part A, B's BFS terminates at its own seed and B
contributes nothing to the output.

Repro — `A = rect(-73.60, 45.50, -73.55, 45.55)`, `B = rect(-73.590, 45.520, -73.520, 45.525)`
(B's centroid falls inside A):

```
p7 inner=false: got=1369  true union=1479  MISSING=110   (A alone=1369, B alone=260)
p7 inner=true:  got=1225  true union=1291  MISSING=66
```

7% of the geography vanishes, with no error raised.

**Fix:** give each polygon its own `visited` set that gates the geometry test; keep
`accepted` global; always expand from any cell that intersects the polygon. This also
removes the need for the `rejected` set entirely.

*Note: superseded if B1 is fixed by adopting P1 (hierarchical descent), which cannot
exhibit this bug structurally.*

---

### [ ] B2 — `n_hops_for` can produce an unbounded hop count **[M]**

`src/lib.rs:529-542`

`cell_width_m` is scaled by `cos(lat_center)`, so `min_cell_m` collapses toward the poles.
Computed hop counts for `expansion_m = 1000`:

| precision | lat 60 | lat 80 | lat 89 | lat ~90 |
|---|---|---|---|---|
| p7 | 14 | 38 | 376 | 638,868 |
| p9 | 419 | 1,206 | 11,995 | 1.5×10⁸ |

At p9 / lat 80 that is a BFS frontier of millions of cells. If `min_cell_m` ever reaches 0,
`expansion_m / 0.0` is `+inf`, `as usize` saturates to `usize::MAX`, and
`for _ in 0..n_hops` never terminates.

**Fix:** guard `min_cell_m > 0`, and cap `n_hops` with a descriptive error rather than
letting the BFS run away.

---

### [ ] B3 — `expand_geohash_set` does a full pre-pass even when `n_hops == 0` **[M]**

`src/lib.rs:63-69`

The initial-frontier scan is O(N·8) with 8 `String` allocations per cell, and runs even
when nothing will be expanded. Measured on 40,401 input cells:

| | time |
|---|---|
| current | 22.0 ms |
| early return on `n_hops == 0` | 1.2 ms |

`expand_geohashes(hs, 0.0)` is a legitimate call.

**Caveat:** that pre-pass is currently the only thing that validates input hashes, so an
early return changes error behaviour for malformed hashes at 0 m. Decide whether to keep
validating explicitly.

---

### [ ] B4 — `polygon_to_geohashes` never releases the GIL **[M]**

`src/lib.rs:270-315`

The `_py: Python` parameter is unused and `polygons_to_geohashes` runs while holding the
GIL. Verdun at p10 spends 90 s in this function — 90 s with the whole interpreter blocked.
Every other heavy function in the file already uses `py.detach`.

**Fix:** extract the polygons under the GIL, then `py.detach(|| polygons_to_geohashes(...))`.

---

### [ ] B5 — Inconsistent containment semantics between the two `inner=true` paths **[M]**

`src/lib.rs:130-138`

- with holes: `polygon.contains(cell)` — DE-9IM semantics
- without holes: `!exterior.intersects(cell.exterior()) && cell.area <= poly.area`

The hole-free fast path **rejects** a cell that merely touches the polygon boundary from
the inside; DE-9IM `contains` **accepts** it. Same geometry gives a different answer
depending only on whether a hole is present.

**Fix:** pick one semantic. Adopting P1 settles this in the DE-9IM direction for all
polygons — confirm against the test suite before merging.

---

### [ ] B6 — `neighbor.to_string()` on an already-owned `String` **[L]**

`src/lib.rs:155` and `src/lib.rs:221`

`neighbor` is already an owned `String` from the `neighbors()` destructuring. 8 wasted heap
allocations + copies per visited cell.

**Fix:** `testing_geohashes.push_back(neighbor);`

---

### [ ] B7 — `polygon.unsigned_area()` recomputed inside the BFS loop **[L]**

`src/lib.rs:137`

O(V) in the polygon's vertex count, evaluated for every candidate cell when `inner=true`.
Hoisting it out of the loop alone: verdun p9 `inner=true` 2524 ms → 2343 ms (~8%).

*Subsumed by P1, which drops this code path entirely.*

---

### [ ] B8 — `handbrake`: envelope-rejected cells are neither recorded nor expanded from **[L]**

`src/lib.rs:199-201`

`if !condition { continue; }` leaves the cell out of both `inner_geohashes` and
`outer_geohashes`, so it is re-enqueued by every neighbour that reaches it (up to 8×
duplicated work).

`polygons_to_geohashes_handbrake` is dead code outside `benches/bench.rs`.

**Fix:** delete the function and its benchmark arms.

---

### [ ] B9 — Minor papercuts **[L]**

- A null inside a geohash list in `expand_geohash_mapping_arrow` reads back as `""` and
  surfaces as `"all geohashes in a group must have the same precision"` — confusing error.
  (`src/lib.rs:687`)
- `expand_geohashes` docstring says the hop count comes from "cell height"; the code uses
  `min(height, width)`. (`src/lib.rs:544-547`)
- Clippy: `items_after_test_module` — `seed_interior_point_fast` sits below `mod tests`.

---

## Performance

### [ ] P1 — Replace the flood-fill with hierarchical descent over the geohash tree **[H]**

`src/lib.rs:87-162`

Instead of testing every target-precision cell against the polygon, descend the geohash
tree from precision 1:

- a cell disjoint from the polygon prunes an entire subtree
- a cell the polygon **contains** emits all 32ⁿ descendants with no further geometry tests
- only boundary-straddling cells subdivide

Use **one `PreparedGeometry::relate` per visited cell** (R*-tree indexed) to get
`is_intersects` and `is_contains` from a single call.

**The two halves only pay off together.** Measured separately:

- plain `Contains` inside the descent → ~6× *slower* than the current code
- `relate` without the descent → a net loss for `inner=false` (943 ms → 1570 ms at
  verdun p9), because `relate` computes the full DE-9IM matrix where `Intersects`
  short-circuits

Combined, with output sets diffed against the current implementation — **identical in
every case**:

| case | current | descent | speedup | cells |
|---|---|---|---|---|
| verdun p7 inner=false | 7.2 ms | 6.4 ms | 1.1× | 431 |
| verdun p7 inner=true | 8.7 ms | 4.5 ms | 1.9× | 308 |
| verdun p8 inner=false | 48.4 ms | 17.4 ms | 2.8× | 12,152 |
| verdun p8 inner=true | 86.4 ms | 16.6 ms | 5.2× | 11,409 |
| verdun p9 inner=false | 943 ms | 121 ms | 7.8× | 378,735 |
| verdun p9 inner=true | 2,524 ms | 121 ms | **20.9×** | 374,719 |
| verdun p10 inner=false | 32.8 s | 2.58 s | 12.7× | 12,067,236 |
| verdun p10 inner=true | 90.4 s | 2.48 s | **36.5×** | 12,043,463 |
| whitehorse p5 inner=false | 2.7 ms | 3.1 ms | 0.9× | 835 |
| whitehorse p5 inner=true | 4.5 ms | 2.5 ms | 1.8× | 673 |
| whitehorse p6 inner=false | 49.1 ms | 18.0 ms | 2.7× | 24,571 |
| whitehorse p6 inner=true | 107 ms | 18.0 ms | 6.0× | 23,688 |
| whitehorse p7 inner=false | 1,472 ms | 146 ms | 10.1× | 774,137 |
| whitehorse p7 inner=true | 3,396 ms | 141 ms | **24.1×** | 768,952 |
| whitehorse p8 inner=false | 53.5 s | 5.48 s | 9.8× | 24,701,369 |
| whitehorse p8 inner=true | 117 s | 5.85 s | **20.0×** | 24,673,107 |

Below p6 it is a wash — the gain scales with precision, because interior cells stop
costing anything.

Bonuses: drops the seed-point machinery entirely (no `seed_interior_point_fast`, no silent
`continue` when no seed is found), structurally cannot exhibit B1, and the top-level
subtrees parallelize cleanly.

Caveat: uses DE-9IM `contains` for all polygons — see B5.

---

### [ ] P2 — The Python boundary dominates the bulk encode/decode functions **[H]**

`src/lib.rs:328-525`

For 500k p7 geohashes:

| | time |
|---|---|
| pure Rust core (`decode_bbox` + `serialize_bbox`, rayon) | **2.7–3.6 ms** |
| `decode_many_to_wkb` called from Python | **40 ms** |

~92% of wall time is `list[str] → Vec<String>` on the way in and `Vec<Vec<u8>>` → 500k
Python `bytes` objects on the way out. This also explains why the `num_threads` knob does
nothing here: 40.0 ms with the default pool vs 44.3 ms with `num_threads=8` — noise on a
3 ms core.

**Fix:** extend the Arrow pattern already used by `expand_geohash_mapping_arrow` to
`decode_many_to_wkb` / `decode_many` / `encode_many`. Expect 5–8× end to end.

Likely worth more than P1 if the DuckDB / PostGIS ingestion path is the hot one.

---

### [ ] P3 — Integer geohash keys in `expand_geohash_set` **[M]**

`src/lib.rs:49-83`

Pack the base32 hash into a `u64` and compute neighbours by Morton de/re-interleaving
instead of allocating 8 `String`s per cell; `FxHashSet<u64>` instead of `HashSet<String>`.
Measured on 40,401 cells, output identical:

| n_hops | current | u64 |
|---|---|---|
| 1 | 23.8 ms | 6.1 ms |
| 4 | 24.9 ms | 6.3 ms |
| 16 | 36.4 ms | 7.3 ms |

The same helpers (`gh_pack` / `gh_unpack` / `gh_bbox` / `gh_neighbors`) serve P1.

---

### [ ] P4 — `expand_geohash_mapping_arrow` repeats `geog_id` once per row **[L]**

`src/lib.rs:730-750`

For 100k geographies × 500 cells, the `geog_id` column alone is ~1 GB of `LargeUtf8`.
Dictionary or run-end encoding would cut it roughly 5×. Schema change — only worth it if
both ends of the pipe are under your control.

---

### [ ] P5 — `polygon_to_geohashes` is single-threaded **[L]**

`rayon` is already a dependency but the polygon path never uses it. With P1 in place the
32 top-level subtrees (and the parts of a MultiPolygon) fan out cleanly.

Note: `PreparedGeometry` holds an `Rc` and is therefore `!Send` — build one per worker
rather than sharing.

---

## Suggested order

1. B1 (correctness, ships independently)
2. P1 + P3 (share the integer-geohash helpers; P1 subsumes B5, B7 and makes B1 structural)
3. B2, B3, B4
4. P2 (largest win for the bulk API, independent of everything above)
5. B6, B8, B9, P4, P5
