//! Shared board-shape interface for Waffle Iron and Circuit Composer.
//!
//! Exchanges a PCB's outer outline, inner cutouts, board-level drilled holes, component keepouts,
//! and revision/driver handover state as a pair of files:
//! - `board-shape.dxf`: DXF R12 (`AC1009`), millimetres, **Y up**, layers `BOARD_OUTLINE`,
//!   `BOARD_CUTOUT`, `BOARD_HOLE`, `BOARD_KEEPOUT`, with extended entity data under
//!   `BOARD_INTERFACE` registered in an `APPID` table.
//! - `board-shape.json`: schema, board name, revision, parent, author, driver, handover record,
//!   board thickness, canonical nanometre geometry hash, and `approximate` flag.
//!
//! All coordinates and diameters in [`Record`] are whole nanometres (`i64`) in KiCad's board frame
//! (`+X` right, `+Y` down). The crate performs no I/O, reads no system clock, uses no `HashMap`
//! iteration, and compiles cleanly for `wasm32-unknown-unknown`.

pub mod compare;
mod dxf_read;
mod dxf_write;
pub mod error;
pub mod hash;
pub mod model;

pub use compare::{
    Change, ChangeKind, Classification, Comparison, RematchedId, Role, UncertainMatch, compare,
    within,
};
pub use dxf_read::read;
pub use dxf_write::write;
pub use error::Error;
pub use hash::{HASH_PREFIX, canonicalize_loop, hash};
pub use model::{
    APP_ID, Author, Driver, Element, ElementKind, Handover, Hole, Keepout, LAYER_CUTOUT,
    LAYER_HOLE, LAYER_KEEPOUT, LAYER_OUTLINE, Loop, Manifest, NM_PER_MM, Point, Record,
    SCHEMA_VERSION, Side,
};

#[cfg(test)]
mod unit_tests {
    use crate::dxf_write::{format_picometres, format_real, wrap_dxf_degrees};

    #[test]
    fn dxf_real_formatting_pins_nine_decimals_no_exponents_and_unsigned_zero() {
        assert_eq!(format_real(0.0), "0.000000000\n");
        assert_eq!(format_real(-0.0), "0.000000000\n");
        // Sub-half-picometre negative float must ALSO format as unsigned 0.000000000, not -0.000000000.
        assert_eq!(format_real(-1e-12), "0.000000000\n");
        assert_eq!(format_real(1e-7), "0.000000100\n");
        assert_eq!(format_real(-1e-7), "-0.000000100\n");
        assert_eq!(format_real(1e-9), "0.000000001\n");
        assert_eq!(format_real(123456.789012345), "123456.789012345\n");
        for v in [
            0.0,
            -0.0,
            -1e-15,
            1e-9,
            1e-7,
            -1e-7,
            1e6,
            -1e6,
            359.999999999,
        ] {
            let s = format_real(v);
            assert!(
                !s.contains('e') && !s.contains('E'),
                "exponent found in {s:?}"
            );
            assert!(!s.starts_with("-0.000000000"), "signed zero in {s:?}");
            let trimmed = s.trim();
            let (_, frac) = trimmed.split_once('.').expect("decimal point");
            assert_eq!(frac.len(), 9, "expected 9 decimals in {s:?}");
        }

        assert_eq!(format_picometres(0), "0.000000000\n");
        assert_eq!(format_picometres(-500), "-0.000000500\n");
        assert_eq!(format_picometres(1_550_000_500), "1.550000500\n");
        assert_eq!(wrap_dxf_degrees(-0.0), 0.0);
        assert_eq!(wrap_dxf_degrees(360.0), 0.0);
        assert_eq!(wrap_dxf_degrees(359.9999999999), 0.0);
    }
}
