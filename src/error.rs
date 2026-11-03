use std::fmt;

use crate::model::Point;

/// Typed error returned by [`crate::read`], with a dedicated variant for each kind of malformed
/// DXF or manifest input.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Error {
    /// Either `dxf` or `manifest` is not valid UTF-8 (for example, a binary DXF file).
    InvalidUtf8 { which: &'static str },
    /// `board-shape.json` is not valid JSON or does not match the manifest structure.
    InvalidManifestJson { message: String },
    /// `manifest.schema` is not a supported version (`1`).
    UnsupportedSchema { found: u32 },
    /// A required string field in `manifest` (`board`, `revision`, `geometry_hash`,
    /// `author.source`, `author.at`, or `last_handover.at`) is empty or whitespace-only.
    EmptyManifestField { field: &'static str },
    /// `manifest.thickness_mm` is non-finite or non-positive (`<= 0`).
    InvalidThickness { value: String },
    /// DXF stream has an odd number of lines (a group code with no following value line).
    DxfOddLineCount,
    /// A DXF group code line could not be parsed as an integer.
    DxfInvalidGroupCode { line: usize, text: String },
    /// A DXF numeric value (coordinate, radius, angle, or integer flag) is invalid or non-finite.
    DxfInvalidValue {
        line: usize,
        code: i32,
        text: String,
    },
    /// DXF file has no properly closed `ENTITIES` section or is missing its `EOF` marker.
    DxfMissingEntities,
    /// A `POLYLINE` entity has no matching `SEQEND`, or a `VERTEX`/`SEQEND` appeared outside a
    /// `POLYLINE`.
    DxfUnclosedPolyline,
    /// An entity on a `BOARD_*` layer uses an unsupported DXF entity kind for that layer.
    UnsupportedEntity { layer: String, kind: String },
    /// Degenerate entity geometry (zero-length line, non-positive radius, zero-sweep or
    /// non-concentric arc, or polyline with fewer than two distinct vertices).
    DegenerateGeometry { layer: String, detail: String },
    /// Malformed `BOARD_INTERFACE` extended entity data (`XDATA`).
    InvalidXdata { layer: String, detail: String },
    /// Two entities carry the same stable `id` in `BOARD_INTERFACE` extended data.
    DuplicateId { id: String },
    /// No entities were found on `BOARD_OUTLINE`.
    MissingOutline,
    /// Segments on a loop layer (`BOARD_OUTLINE`, `BOARD_CUTOUT`, `BOARD_KEEPOUT`) do not form
    /// closed loops (an endpoint has degree 1).
    OpenLoop {
        layer: String,
        at: Point,
        gap_nm: i64,
    },
    /// Three or more segments meet at a single vertex on a loop layer.
    BranchingLoop {
        layer: String,
        at: Point,
        degree: usize,
    },
    /// `BOARD_OUTLINE` contains more than one closed loop.
    MultipleOutlines { count: usize },
    /// A `BOARD_CUTOUT` loop lies outside `BOARD_OUTLINE` or is nested inside another cutout.
    CutoutOutsideOutline,
    /// Segments grouped into the same `BOARD_KEEPOUT` loop carry conflicting `side` or
    /// `max_height_mm` attributes.
    InconsistentKeepoutAttributes { loop_index: usize },
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidUtf8 { which } => write!(f, "{which} input is not valid UTF-8"),
            Self::InvalidManifestJson { message } => write!(f, "invalid manifest JSON: {message}"),
            Self::UnsupportedSchema { found } => {
                write!(f, "unsupported manifest schema {found} (expected 1)")
            }
            Self::EmptyManifestField { field } => {
                write!(f, "manifest field `{field}` must not be empty")
            }
            Self::InvalidThickness { value } => {
                write!(f, "invalid board thickness_mm `{value}` (must be > 0)")
            }
            Self::DxfOddLineCount => {
                write!(f, "DXF has an odd number of lines (unpaired group code)")
            }
            Self::DxfInvalidGroupCode { line, text } => {
                write!(
                    f,
                    "DXF group code `{text}` at line {line} is not an integer"
                )
            }
            Self::DxfInvalidValue { line, code, text } => {
                write!(
                    f,
                    "invalid DXF value `{text}` for group code {code} at line {line}"
                )
            }
            Self::DxfMissingEntities => {
                write!(f, "DXF is missing a terminated ENTITIES section or EOF")
            }
            Self::DxfUnclosedPolyline => {
                write!(f, "DXF POLYLINE/VERTEX/SEQEND sequence is malformed")
            }
            Self::UnsupportedEntity { layer, kind } => {
                write!(f, "unsupported DXF entity `{kind}` on layer `{layer}`")
            }
            Self::DegenerateGeometry { layer, detail } => {
                write!(f, "degenerate geometry on layer `{layer}`: {detail}")
            }
            Self::InvalidXdata { layer, detail } => {
                write!(
                    f,
                    "invalid BOARD_INTERFACE extended data on `{layer}`: {detail}"
                )
            }
            Self::DuplicateId { id } => {
                write!(f, "duplicate element id `{id}`")
            }
            Self::MissingOutline => {
                write!(f, "no entities on layer BOARD_OUTLINE")
            }
            Self::OpenLoop { layer, at, gap_nm } => {
                write!(
                    f,
                    "unclosed loop on `{layer}` at ({}, {}) nm (nearest gap {gap_nm} nm)",
                    at.x, at.y
                )
            }
            Self::BranchingLoop { layer, at, degree } => {
                write!(
                    f,
                    "branching loop on `{layer}` at ({}, {}) nm ({degree} segments meet)",
                    at.x, at.y
                )
            }
            Self::MultipleOutlines { count } => {
                write!(f, "expected 1 outer loop on BOARD_OUTLINE, found {count}")
            }
            Self::CutoutOutsideOutline => {
                write!(
                    f,
                    "cutout on BOARD_CUTOUT is not strictly inside BOARD_OUTLINE (or is nested)"
                )
            }
            Self::InconsistentKeepoutAttributes { loop_index } => {
                write!(
                    f,
                    "conflicting side or max_height_mm attributes in BOARD_KEEPOUT loop {loop_index}"
                )
            }
        }
    }
}

impl std::error::Error for Error {}
