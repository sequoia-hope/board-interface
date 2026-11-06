use std::cmp::Ordering;

use serde::{Deserialize, Serialize};

/// Current `board-shape.json` schema version.
pub const SCHEMA_VERSION: u32 = 1;

/// Registered DXF `APPID` name carrying extended entity data.
pub const APP_ID: &str = "BOARD_INTERFACE";

/// DXF layer for the single closed outer board outline.
pub const LAYER_OUTLINE: &str = "BOARD_OUTLINE";

/// DXF layer for closed inner board cutouts.
pub const LAYER_CUTOUT: &str = "BOARD_CUTOUT";

/// DXF layer for board-level drilled holes (`CIRCLE` entities).
pub const LAYER_HOLE: &str = "BOARD_HOLE";

/// DXF layer for closed component keepout loops.
pub const LAYER_KEEPOUT: &str = "BOARD_KEEPOUT";

/// Whole nanometres per millimetre (`1 mm = 1_000_000 nm`).
pub const NM_PER_MM: i64 = 1_000_000;

/// A 2D point in whole nanometres in KiCad's board frame (`+X` right, `+Y` down).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct Point {
    pub x: i64,
    pub y: i64,
}

impl Point {
    #[inline]
    pub const fn new(x: i64, y: i64) -> Self {
        Self { x, y }
    }
}

impl From<[i64; 2]> for Point {
    fn from([x, y]: [i64; 2]) -> Self {
        Self { x, y }
    }
}

impl From<(i64, i64)> for Point {
    fn from((x, y): (i64, i64)) -> Self {
        Self { x, y }
    }
}

impl From<Point> for [i64; 2] {
    fn from(p: Point) -> Self {
        [p.x, p.y]
    }
}

/// Geometric curve kind of a loop element in KiCad's Y-down nanometre frame.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ElementKind {
    Line {
        start: Point,
        end: Point,
    },
    Arc {
        center: Point,
        start: Point,
        end: Point,
        /// `true` when the sweep from `start` to `end` increases the coordinate angle
        /// `atan2(y - center.y, x - center.x)` in KiCad's Y-down frame (visually clockwise on a
        /// Y-down screen). Flipping Y to DXF's Y-up frame (`(x, -y)`) reverses sweep sense:
        /// `angle_increasing: true` becomes clockwise in Y-up, so a counter-clockwise DXF `ARC` runs from
        /// `(end.x, -end.y)` (group 50) to `(start.x, -start.y)` (group 51).
        angle_increasing: bool,
    },
    Circle {
        center: Point,
        diameter_nm: i64,
    },
}

impl ElementKind {
    /// Reverse traversal direction of this curve. Circles are unchanged; arcs swap `start` and
    /// `end` and invert `angle_increasing`.
    pub fn reversed(&self) -> Self {
        match *self {
            Self::Line { start, end } => Self::Line {
                start: end,
                end: start,
            },
            Self::Arc {
                center,
                start,
                end,
                angle_increasing,
            } => Self::Arc {
                center,
                start: end,
                end: start,
                angle_increasing: !angle_increasing,
            },
            Self::Circle {
                center,
                diameter_nm,
            } => Self::Circle {
                center,
                diameter_nm,
            },
        }
    }

    /// Direction-independent canonical form of a single curve.
    pub fn undirected_canonical(&self) -> Self {
        let rev = self.reversed();
        if *self <= rev { *self } else { rev }
    }

    /// Start and end points for open segments (`Line` and `Arc`), or `None` for `Circle`.
    pub fn endpoints(&self) -> Option<(Point, Point)> {
        match *self {
            Self::Line { start, end } | Self::Arc { start, end, .. } => Some((start, end)),
            Self::Circle { .. } => None,
        }
    }
}

/// One element of a [`Loop`], pairing its optional stable namespaced ID with its geometry.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct Element {
    pub id: Option<String>,
    pub kind: ElementKind,
}

impl Element {
    pub fn new(id: Option<String>, kind: ElementKind) -> Self {
        Self { id, kind }
    }

    /// Reverse traversal direction while preserving the element's ID.
    pub fn reversed(&self) -> Self {
        Self {
            id: self.id.clone(),
            kind: self.kind.reversed(),
        }
    }
}

impl PartialOrd for Element {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for Element {
    fn cmp(&self, other: &Self) -> Ordering {
        (&self.kind, &self.id).cmp(&(&other.kind, &other.id))
    }
}

/// A closed boundary loop of elements in walk order.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct Loop {
    pub elements: Vec<Element>,
}

impl Loop {
    pub fn new(elements: Vec<Element>) -> Self {
        Self { elements }
    }

    /// Reverse walk direction of the entire loop, reversing each segment's endpoints and arc
    /// sweep flag.
    pub fn reversed(&self) -> Self {
        Self {
            elements: self.elements.iter().rev().map(Element::reversed).collect(),
        }
    }
}

/// A board-level drilled hole in KiCad's Y-down nanometre frame.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct Hole {
    pub id: Option<String>,
    pub center: Point,
    pub diameter_nm: i64,
    pub plated: bool,
    pub clearance_nm: i64,
}

impl PartialOrd for Hole {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for Hole {
    fn cmp(&self, other: &Self) -> Ordering {
        (
            self.center,
            self.diameter_nm,
            self.plated,
            self.clearance_nm,
            &self.id,
        )
            .cmp(&(
                other.center,
                other.diameter_nm,
                other.plated,
                other.clearance_nm,
                &other.id,
            ))
    }
}

/// Board side to which a keepout applies.
#[derive(
    Debug, Clone, Copy, Default, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize,
)]
#[serde(rename_all = "lowercase")]
pub enum Side {
    #[default]
    Both,
    Front,
    Back,
}

impl Side {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Both => "both",
            Self::Front => "front",
            Self::Back => "back",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "both" => Some(Self::Both),
            "front" => Some(Self::Front),
            "back" => Some(Self::Back),
            _ => None,
        }
    }
}

/// A closed keepout region with its board side and optional component height limit.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct Keepout {
    pub boundary: Loop,
    pub side: Side,
    /// Maximum allowed component height in whole nanometres (`Some(0)` forbids all components;
    /// `None` means no height cap was specified).
    pub max_height_nm: Option<i64>,
}

/// Suite application or external tool driving or authoring the board outline.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Driver {
    Waffle,
    Composer,
    Kicad,
}

impl Driver {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Waffle => "waffle",
            Self::Composer => "composer",
            Self::Kicad => "kicad",
        }
    }
}

/// Provenance of the commit or workspace revision that wrote `board-shape.json`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Author {
    pub app: Driver,
    pub source: String,
    pub at: String,
}

/// Record of the most recent drive handover ("unlock at your peril").
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Handover {
    pub by: Driver,
    pub at: String,
    /// The revision the drive was taken over from, which must be the previous driver's latest.
    /// A follower can see a handover it missed from this, however many revisions came after it.
    pub from_revision: String,
}

/// `board-shape.json` manifest accompanying `board-shape.dxf`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Manifest {
    pub schema: u32,
    pub board: String,
    /// Derived by [`crate::revision()`] from `geometry_hash`, `driver`, `parent` and
    /// `last_handover`. Set it with [`crate::seal`]; never set it to the geometry hash.
    pub revision: String,
    /// The revision this one was published on top of, or `None` for the first.
    pub parent: Option<String>,
    pub author: Author,
    pub driver: Driver,
    pub last_handover: Option<Handover>,
    pub thickness_mm: f64,
    pub geometry_hash: String,
    pub approximate: bool,
}

impl Manifest {
    /// Board thickness rounded to whole nanometres.
    pub fn thickness_nm(&self) -> i64 {
        (self.thickness_mm * (NM_PER_MM as f64)).round() as i64
    }
}

/// Neutral board-shape record in whole nanometres in KiCad's frame (`+X` right, `+Y` down).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Record {
    pub outline: Loop,
    pub cutouts: Vec<Loop>,
    pub holes: Vec<Hole>,
    pub keepouts: Vec<Keepout>,
    pub manifest: Manifest,
}

impl Record {
    /// Board thickness in whole nanometres.
    pub fn thickness_nm(&self) -> i64 {
        self.manifest.thickness_nm()
    }
}
