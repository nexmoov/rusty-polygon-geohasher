# Code review: bugs & performance — checklist

Findings from a review of `src/lib.rs` (v0.6.4, commit `c4a9e30`).
All measurements taken on `tests/data/{verdun,whitehorse}_wkt.txt`, release build, Apple Silicon.
Prototypes that produced the numbers live in the session scratchpad under `perf/`.

Legend: **[H]** high / **[M]** medium / **[L]** low.

---

## Bugs

### [x] B1 — MultiPolygon parts silently dropped

> **Done** — `fix/multipolygon-dropped-parts` (PR1). **[H]**

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

### [x] B2 — `n_hops_for` can produce an unbounded hop count

> **Done** — `fix/hop-count-guard` (PR4). **[M]**

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

### [x] B3 — `expand_geohash_set` does a full pre-pass even when `n_hops == 0`

> **Done** — `perf/expand-integer-keys` (PR3). **[M]**

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

### [x] B4 — `polygon_to_geohashes` never releases the GIL

> **Done** — `fix/release-gil-polygon` (PR5). **[M]**

`src/lib.rs:270-315`

The `_py: Python` parameter is unused and `polygons_to_geohashes` runs while holding the
GIL. Verdun at p10 spends 90 s in this function — 90 s with the whole interpreter blocked.
Every other heavy function in the file already uses `py.detach`.

**Fix:** extract the polygons under the GIL, then `py.detach(|| polygons_to_geohashes(...))`.

---

### [x] B5 — Inconsistent containment semantics between the two `inner=true` paths

> **Done** — `perf/hierarchical-descent` (PR2). **[M]**

`src/lib.rs:130-138`

- with holes: `polygon.contains(cell)` — DE-9IM semantics
- without holes: `!exterior.intersects(cell.exterior()) && cell.area <= poly.area`

The hole-free fast path **rejects** a cell that merely touches the polygon boundary from
the inside; DE-9IM `contains` **accepts** it. Same geometry gives a different answer
depending only on whether a hole is present.

**Fix:** pick one semantic. Adopting P1 settles this in the DE-9IM direction for all
polygons — confirm against the test suite before merging.

---

### [x] B6 — `neighbor.to_string()` on an already-owned `String`

> **Done** — `fix/multipolygon-dropped-parts` (PR1). **[L]**

`src/lib.rs:155` and `src/lib.rs:221`

`neighbor` is already an owned `String` from the `neighbors()` destructuring. 8 wasted heap
allocations + copies per visited cell.

**Fix:** `testing_geohashes.push_back(neighbor);`

---

### [x] B7 — `polygon.unsigned_area()` recomputed inside the BFS loop

> **Done** — `fix/multipolygon-dropped-parts` (PR1). **[L]**

`src/lib.rs:137`

O(V) in the polygon's vertex count, evaluated for every candidate cell when `inner=true`.
Hoisting it out of the loop alone: verdun p9 `inner=true` 2524 ms → 2343 ms (~8%).

*Subsumed by P1, which drops this code path entirely.*

---

### [x] B8 — `handbrake`: envelope-rejected cells are neither recorded nor expanded from

> **Done** — `chore/remove-dead-code` (PR6). **[L]**

`src/lib.rs:199-201`

`if !condition { continue; }` leaves the cell out of both `inner_geohashes` and
`outer_geohashes`, so it is re-enqueued by every neighbour that reaches it (up to 8×
duplicated work).

`polygons_to_geohashes_handbrake` is dead code outside `benches/bench.rs`.

**Fix:** delete the function and its benchmark arms.

---

### [x] B9 — Minor papercuts

> **Done** — `chore/remove-dead-code` (PR6). **[L]**

- A null inside a geohash list in `expand_geohash_mapping_arrow` reads back as `""` and
  surfaces as `"all geohashes in a group must have the same precision"` — confusing error.
  (`src/lib.rs:687`)
- `expand_geohashes` docstring says the hop count comes from "cell height"; the code uses
  `min(height, width)`. (`src/lib.rs:544-547`)
- Clippy: `items_after_test_module` — `seed_interior_point_fast` sits below `mod tests`.

---

## Performance

### [x] P1 — Replace the flood-fill with hierarchical descent over the geohash tree

> **Done** — `perf/hierarchical-descent` (PR2). **[H]**

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

### [x] P2 — The Python boundary dominates the bulk encode/decode functions **[H]**

> **Done** — `perf/arrow-wkb-output` (PR8) and `perf/arrow-codec-coords` (PR9).

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

Fixed by adding Arrow twins alongside the list API, which is untouched.
Measured at N = 500,000, precision 7, medians of 15 runs:

| function | list API | Arrow | speedup |
|---|---|---|---|
| `decode_many_to_wkb` | 45.0 ms | 3.1 ms | **14.5×** |
| `decode_many_to_ewkb` | 51.8 ms | 3.2 ms | **16.2×** |
| `encode_many` | 24.8 ms | 3.4 ms | 7.3× |
| `decode_many` | 52.8 ms | 1.9 ms | **27.8×** |
| `decode_many_exactly` | 68.8 ms | 2.8 ms | **24.6×** |

Even when the data starts as a Python list and must be converted first,
`list → pa.array → decode_many_to_wkb_arrow` is 10.5 ms, still 4.3×. Output matches the
list API exactly in every case.

Notes on the implementation:

- Every row of a WKB/EWKB or geohash column is a fixed width, so the values buffer is
  allocated once for the whole column and filled across threads in place, removing the
  per-row `Vec`.
- Inputs accept `Utf8` or `LargeUtf8`, so callers need not match whichever offset width
  the library happens to prefer.
- Nulls propagate rather than being rejected or decoded as an empty string.
- The decoders return a `RecordBatch` — `(lng, lat)` and `(lng, lat, lng_err, lat_err)` —
  which is the shape a dataframe or DuckDB table wants, and keeps each column contiguous.

---

### [x] P3 — Integer geohash keys in `expand_geohash_set`

> **Done** — `perf/expand-integer-keys` (PR3). **[M]**

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

## Branch stack

Each branch is built on the one above it, so they merge bottom-up. Local only
— nothing has been pushed.

```
main
 ├─ docs/code-review-checklist          this file
 └─ fix/multipolygon-dropped-parts      PR1  B1, B6, B7
     └─ perf/hierarchical-descent       PR2  P1, B5      (+ ghbits)
         └─ perf/expand-integer-keys    PR3  P3, B3
             └─ fix/hop-count-guard     PR4  B2
                 └─ fix/release-gil-polygon    PR5  B4
                     └─ chore/remove-dead-code PR6  B8, B9
                         └─ chore/rustfmt      PR7  formatting only
                             └─ perf/arrow-wkb-output       PR8  P2 (WKB/EWKB)
                                 └─ perf/arrow-codec-coords PR9  P2 (encode/decode)
```

Verified at the tip of the stack: 58 Rust tests, 165 Python tests,
`cargo clippy --all-targets` silent, `cargo fmt --check` clean.

## Still open

- **P4** — dictionary-encode the repeated `geog_id` column.
- **P5** — parallelise the polygon cover across subtrees.
- `seed_interior_point_fast` is now unused inside the crate, since the descent
  needs no seed point. It is still `pub`, so removing it is a breaking change
  for any Rust consumer — left in place pending a call on that.
