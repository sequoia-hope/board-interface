# Golden files

Shared test fixtures for `board-interface`, run in CI by both Waffle Iron and Circuit Composer.
Each fixture is a pair of files: `<name>.dxf` (`board-shape.dxf`, DXF R12 / `AC1009`, millimetres,
Y up, `BOARD_INTERFACE` extended data) and `<name>.json` (`board-shape.json`, schema `1` manifest).

## Valid boards

| Fixture | Geometry & features | How it was made |
| --- | --- | --- |
| `round_board.{dxf,json}` | Circular 50 mm diameter board (`BOARD_OUTLINE` `CIRCLE`), one circular 12 mm inner cutout (`BOARD_CUTOUT` `CIRCLE`), four M2.5 mounting holes (`2.7 mm` drill, two plated with `0.25 mm` clearance, two non-plated with `0.3 mm` clearance), and one circular `back`-side keepout (`2.5 mm` height limit). | Built in `tests/integration.rs` (`build_round_board`) and emitted via `board_interface::write` (`UPDATE_GOLDEN=1 cargo test`). |
| `rounded_rect.{dxf,json}` | `80 × 50 mm` board with four `4 mm`-radius `ARC` corners and four `LINE` edges on `BOARD_OUTLINE`, one `10 × 6 mm` rectangular `BOARD_CUTOUT`, two `3.2 mm` mounting holes on `BOARD_HOLE`, and one rectangular `front`-side `BOARD_KEEPOUT` (`height = 0`, no components). | Built in `tests/integration.rs` (`build_rounded_rect`) and emitted via `board_interface::write` (`UPDATE_GOLDEN=1 cargo test`). |
| `easy2350.{dxf,json}` | `21 × 51 mm` RP2350 board (`[100, 121] × [100, 151] mm` in KiCad sheet coordinates, `1.6 mm` thickness) with four `LINE` edges and four `1 mm`-radius `ARC` corner fillets, carrying `kicad:<tstamp>` element IDs. Canonical hash: `sha256:d46bc4bcff1ed485d79e6a47a8a521c1c193ee21a64e6d6ec811e0dd253a0088`. | Converted from the `Edge.Cuts` layer of `easy2350.kicad_pcb` by `python3 tools/convert_easy2350.py` (see provenance below) and verified byte-for-byte against `board_interface::write`. |

### Provenance of `easy2350`

- Source file (read-only): `boardroom/examples/easy2350/hw/easy2350/a/easy2350.kicad_pcb`
- Last commit touching `easy2350.kicad_pcb`: `56d2bd8fcb94ff5f1137e66a203efa30dc7c91e4` (`2026-11-02 23:58:12 -0800`)
- `boardroom` repository `HEAD` at conversion: `2352e6217457289105e67aba9458d49d88342eb3`
- Converter command:

```bash
python3 tools/convert_easy2350.py
```

## Malformed inputs (`golden/errors/`)

One `<prefix>.dxf` + `<prefix>.json` pair per variant of `board_interface::Error`, generated
deterministically by `build_error_cases` in `tests/integration.rs` (`UPDATE_GOLDEN=1 cargo test`)
and checked by `golden_error_files_cover_every_typed_error_variant`:

| Fixture prefix | `Error` variant | Malformed condition |
| --- | --- | --- |
| `01_invalid_utf8` | `Error::InvalidUtf8` | Non-UTF-8 byte sequence (`0xFF 0xFE ...`) in DXF bytes |
| `02_invalid_manifest_json` | `Error::InvalidManifestJson` | Truncated JSON syntax in `board-shape.json` |
| `03_unsupported_schema` | `Error::UnsupportedSchema` | `schema: 99` instead of `1` |
| `04_empty_manifest_field` | `Error::EmptyManifestField` | Empty `"board": ""` string in manifest |
| `05_invalid_thickness` | `Error::InvalidThickness` | Negative `"thickness_mm": -1.6` |
| `06_dxf_odd_line_count` | `Error::DxfOddLineCount` | Trailing group code without a value line |
| `07_dxf_invalid_group_code` | `Error::DxfInvalidGroupCode` | Non-integer group code `"NOT_A_CODE"` |
| `08_dxf_invalid_value` | `Error::DxfInvalidValue` | Non-numeric coordinate `"not_a_float"` on group `10` |
| `09_dxf_missing_entities` | `Error::DxfMissingEntitiesSection` | DXF containing `HEADER` only and no `ENTITIES` section |
| `10_dxf_unclosed_polyline` | `Error::DxfUnclosedPolyline` | `POLYLINE` entity missing its terminating `SEQEND` |
| `11_unsupported_entity` | `Error::UnsupportedEntity` | `SPLINE` entity on `BOARD_OUTLINE` |
| `12_degenerate_geometry` | `Error::DegenerateGeometry` | Zero-length `LINE` (`start == end`) on `BOARD_OUTLINE` |
| `13_invalid_xdata` | `Error::InvalidXdata` | Unknown `BOARD_INTERFACE` XDATA key `"bogus_key"` |
| `14_duplicate_id` | `Error::DuplicateId` | Two entities sharing `"id": "dup:1"` |
| `15_missing_outline` | `Error::MissingOutline` | Only `BOARD_HOLE` present; nothing on `BOARD_OUTLINE` |
| `16_open_loop` | `Error::OpenLoop` | Unclosed two-segment L-corner on `BOARD_OUTLINE` |
| `17_branching_loop` | `Error::BranchingLoop` | Degree-3 vertex (`4` segments meeting at `(0, 0)`) on `BOARD_OUTLINE` |
| `18_multiple_outlines` | `Error::MultipleOutlines` | Two disjoint circles on `BOARD_OUTLINE` |
| `19_cutout_outside_outline` | `Error::CutoutOutsideOutline` | `BOARD_CUTOUT` circle placed outside `BOARD_OUTLINE` |
| `20_inconsistent_keepout_attributes` | `Error::InconsistentKeepoutAttributes` | Segments of the same `BOARD_KEEPOUT` loop carrying conflicting `side` (`front` vs `back`) |
