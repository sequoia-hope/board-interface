# board-interface

Reads, writes, hashes and compares `board-shape.dxf` and `board-shape.json`, the shared board
outline files exchanged between Waffle Iron and Circuit Composer. MIT licensed.

It is a small pure-Rust library (`edition = "2024"`, Rust `1.97.1`, `wasm32-unknown-unknown`
ready) with no I/O, no clock and no `HashMap` iteration. Callers pass bytes and timestamps in, and
get deterministic output back.

## The neutral record

Coordinates in memory are `i64` whole nanometres (`1 mm = 1_000_000 nm`) in KiCad's board frame
(`+X` right, `+Y` down):

```rust
use board_interface::{read, write, hash, seal, compare, within};

let record = read(dxf_bytes, manifest_bytes)?;
let digest = hash(&record);              // "sha256:..."

// Publishing: set driver, parent and last_handover, then seal, then write.
let mut next = record.clone();
next.manifest.parent = Some(record.manifest.revision.clone());
seal(&mut next);                         // geometry_hash, then revision "rev:sha256:..."
let (dxf_out, json_out) = write(&next);

if !within(&previous, &record, 1) {
    let diff = compare(&previous, &record);
    // diff.classification, diff.changes, diff.rematched, diff.uncertain
}
```

| Type | What it holds |
| --- | --- |
| `Point` | `x` and `y` in `i64` nanometres (`+X` right, `+Y` down) |
| `ElementKind` | `Line { start, end }`, `Arc { center, start, end, angle_increasing }`, or `Circle { center, diameter_nm }` |
| `Element` | optional namespaced `id` (`"waffle:..."`, `"composer:..."`, `"kicad:..."`) plus `ElementKind` |
| `Loop` | closed boundary walk of `Element`s (one `Circle` or `>= 2` connected open segments) |
| `Hole` | `id`, `center`, `diameter_nm`, `plated`, `clearance_nm` |
| `Keepout` | `boundary: Loop`, `side` (`Both`, `Front`, `Back`), `max_height_nm` |
| `Manifest` | `schema`, `board`, `revision`, `parent`, `author`, `driver`, `last_handover`, `thickness_mm`, `geometry_hash`, `approximate` |
| `Record` | `outline`, `cutouts`, `holes`, `keepouts`, and `manifest` |

## DXF R12 wire format

`write(&record)` emits DXF R12 (`AC1009`) in millimetres with **Y up** (`(x, -y)`):

- **Layers**: `BOARD_OUTLINE`, `BOARD_CUTOUT`, `BOARD_HOLE`, `BOARD_KEEPOUT`.
- **Entities**: `LINE`, `ARC` (counter-clockwise in DXF Y-up, reversing sweep across the Y flip),
  and `CIRCLE`.
- **Formatting**: nine fixed decimal places on all reals, never scientific notation, and `-0.0`
  formatted as `0.000000000`.
- **Extended data (`BOARD_INTERFACE`)**: registered in `APPID`, carrying stable element IDs, loop
  indices, hole `plated` and `clearance`, keepout `side` and `height`, and exact `ARC` endpoints
  and Y-down direction so arcs round-trip to `0 nm`.

`read(dxf, manifest)` also reads files re-saved by external CAD tools: missing `BOARD_INTERFACE`
extended data leaves IDs as `None` (welding loop corners within `100 nm`), and closed `POLYLINE` /
`LWPOLYLINE` entities with vertex bulges are converted into `Line` and `Arc` elements.

## Hashing, tolerance and handover comparison

- `hash(&record)` normalizes every loop for walk direction (flipping `Arc::angle_increasing` when reversed) and
  starting element, sorts cutouts, holes and keepouts canonically, hashes carried `Arc` endpoints
  directly in `i64` nanometres alongside IDs, attributes and board thickness, and returns
  `"sha256:<hex>"`.
- `within(a, b, tolerance_nm)` checks whether two records agree within `tolerance_nm` so sub-nm or
  1 nm rounding noise does not republish a board.
- `revision(&manifest)` derives a manifest's `revision` from its `geometry_hash`, `driver`,
  `parent` and `last_handover`, returning `"rev:sha256:<hex>"`. Every app must use it (or the
  encoding documented on it), so all of them compute the same string. Hashing the parent in means a
  revision always differs from its parent, even for a take over with no edits, and restoring an
  earlier shape does not repeat an earlier revision. `seal(&mut record)` sets `geometry_hash` and
  then `revision`; call it last before `write`. `compare` treats revisions as opaque, so files
  written by `0.1.0`, whose revision was the geometry hash, still compare.
- `compare(base, theirs)` reports per-ID `Added`, `Removed`, `Moved { dx_nm, dy_nm }` and `Changed`
  items (or geometrically re-matches stripped files and flags ambiguous candidates in `uncertain`),
  and classifies handover state into `InSync`, `UpdatedByDriver`, `TheirsBehind`, `CleanHandover`,
  `StaleTakeOver`, `CompetingTakeOvers` or `ChangedOutsideSuite`.

## Checks

```bash
cargo test
cargo clippy --all-targets -- -D warnings
cargo fmt --check
cargo build --target wasm32-unknown-unknown
```
