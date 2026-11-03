use std::collections::{BTreeMap, BTreeSet};

use crate::error::Error;
use crate::hash::{angle_ydown, sweep_ydown};
use crate::model::{
    APP_ID, Element, ElementKind, Hole, Keepout, LAYER_CUTOUT, LAYER_HOLE, LAYER_KEEPOUT,
    LAYER_OUTLINE, Loop, Manifest, Point, Record, SCHEMA_VERSION, Side,
};

/// Weld tolerance (`100 nm = 0.1 µm`) used ONLY when extended data was stripped by a third-party
/// re-save so arc endpoints had to be reconstructed from DXF angles.
const RESAVE_WELD_NM: f64 = 100.0;

/// Read `(board_shape_dxf, board_shape_json)` into a validated [`Record`].
///
/// Each kind of malformed input produces its own [`Error`] variant. Files re-saved by external CAD
/// programs (missing `BOARD_INTERFACE` extended data, or using `POLYLINE` / `LWPOLYLINE` with
/// bulges) are accepted with absent IDs (`None`), allowing [`crate::compare`] to classify them as
/// `ChangedOutsideSuite` and re-match elements by geometry.
pub fn read(dxf: impl AsRef<[u8]>, manifest: impl AsRef<[u8]>) -> Result<Record, Error> {
    let dxf_str =
        std::str::from_utf8(dxf.as_ref()).map_err(|_| Error::InvalidUtf8 { which: "dxf" })?;
    let manifest_str = std::str::from_utf8(manifest.as_ref())
        .map_err(|_| Error::InvalidUtf8 { which: "manifest" })?;

    // Binary DXF starts with "AutoCAD Binary DXF\r\n\x1a\x00" — even if valid UTF-8 prefix,
    // reject explicitly as non-ASCII DXF.
    if dxf_str.starts_with("AutoCAD Binary DXF") {
        return Err(Error::InvalidUtf8 { which: "dxf" });
    }

    let manifest = parse_and_validate_manifest(manifest_str)?;
    let raw_entities = parse_dxf_entities(dxf_str)?;

    let mut seen_ids = BTreeSet::new();
    let mut outline_items: Vec<LoopItem> = Vec::new();
    let mut cutout_items: Vec<LoopItem> = Vec::new();
    let mut keepout_items: Vec<KeepoutItem> = Vec::new();
    let mut holes: Vec<Hole> = Vec::new();

    for raw in raw_entities {
        let layer = raw.layer.as_str();
        if !matches!(
            layer,
            LAYER_OUTLINE | LAYER_CUTOUT | LAYER_HOLE | LAYER_KEEPOUT
        ) {
            // Ignore annotation/construction layers (e.g. "0", "Defpoints") added by external CAD.
            continue;
        }

        let xdata = parse_xdata(layer, &raw.tags)?;
        if let Some(id) = &xdata.id
            && !seen_ids.insert(id.clone())
        {
            return Err(Error::DuplicateId { id: id.clone() });
        }

        if layer == LAYER_HOLE {
            validate_hole_xdata(layer, &xdata)?;
            if raw.kind != "CIRCLE" {
                return Err(Error::UnsupportedEntity {
                    layer: raw.layer,
                    kind: raw.kind,
                });
            }
            let (center, diameter_nm) = parse_circle_geom(layer, &raw.tags)?;
            holes.push(Hole {
                id: xdata.id,
                center,
                diameter_nm,
                plated: xdata.plated.unwrap_or(false),
                clearance_nm: xdata.clearance_nm.unwrap_or(0),
            });
            continue;
        }

        // Loop layers: BOARD_OUTLINE, BOARD_CUTOUT, BOARD_KEEPOUT
        if xdata.plated.is_some() || xdata.clearance_nm.is_some() {
            return Err(Error::InvalidXdata {
                layer: layer.to_string(),
                detail: "hole attribute (plated/clearance_mm) on non-hole layer".to_string(),
            });
        }
        if layer != LAYER_KEEPOUT && (xdata.side.is_some() || xdata.max_height_nm.is_some()) {
            return Err(Error::InvalidXdata {
                layer: layer.to_string(),
                detail: "keepout attribute (side/max_height_mm) on non-keepout layer".to_string(),
            });
        }

        let parsed_segs = parse_loop_entity(&raw, &xdata)?;
        match layer {
            LAYER_OUTLINE => {
                for seg in parsed_segs {
                    outline_items.push(LoopItem {
                        seg,
                        loop_idx: xdata.loop_idx,
                    });
                }
            }
            LAYER_CUTOUT => {
                for seg in parsed_segs {
                    cutout_items.push(LoopItem {
                        seg,
                        loop_idx: xdata.loop_idx,
                    });
                }
            }
            LAYER_KEEPOUT => {
                for seg in parsed_segs {
                    keepout_items.push(KeepoutItem {
                        seg,
                        loop_idx: xdata.loop_idx,
                        side: xdata.side,
                        max_height_nm: xdata.max_height_nm,
                    });
                }
            }
            _ => unreachable!(),
        }
    }

    if outline_items.is_empty() {
        return Err(Error::MissingOutline);
    }

    let outline_segs: Vec<ParsedSeg> = outline_items.into_iter().map(|i| i.seg).collect();
    let mut outline_loops = chain_segments(LAYER_OUTLINE, outline_segs)?;
    if outline_loops.len() != 1 {
        return Err(Error::MultipleOutlines {
            count: outline_loops.len(),
        });
    }
    let outline = outline_loops.remove(0);

    let cutouts = build_cutout_loops(cutout_items)?;
    validate_cutouts_inside_outline(&outline, &cutouts)?;

    let keepouts = build_keepouts(keepout_items)?;

    Ok(Record {
        outline,
        cutouts,
        holes,
        keepouts,
        manifest,
    })
}

fn parse_and_validate_manifest(manifest_str: &str) -> Result<Manifest, Error> {
    let manifest: Manifest =
        serde_json::from_str(manifest_str).map_err(|e| Error::InvalidManifestJson {
            message: e.to_string(),
        })?;

    if manifest.schema != SCHEMA_VERSION {
        return Err(Error::UnsupportedSchema {
            found: manifest.schema,
        });
    }
    if manifest.board.trim().is_empty() {
        return Err(Error::EmptyManifestField { field: "board" });
    }
    if manifest.revision.trim().is_empty() {
        return Err(Error::EmptyManifestField { field: "revision" });
    }
    if manifest.geometry_hash.trim().is_empty() {
        return Err(Error::EmptyManifestField {
            field: "geometry_hash",
        });
    }
    if manifest.author.source.trim().is_empty() {
        return Err(Error::EmptyManifestField {
            field: "author.source",
        });
    }
    if manifest.author.at.trim().is_empty() {
        return Err(Error::EmptyManifestField { field: "author.at" });
    }
    if let Some(parent) = &manifest.parent
        && parent.trim().is_empty()
    {
        return Err(Error::EmptyManifestField { field: "parent" });
    }
    if let Some(handover) = &manifest.last_handover
        && handover.at.trim().is_empty()
    {
        return Err(Error::EmptyManifestField {
            field: "last_handover.at",
        });
    }
    if let Some(handover) = &manifest.last_handover
        && handover.from_revision.trim().is_empty()
    {
        return Err(Error::EmptyManifestField {
            field: "last_handover.from_revision",
        });
    }
    if !manifest.thickness_mm.is_finite()
        || manifest.thickness_mm <= 0.0
        || manifest.thickness_nm() <= 0
    {
        return Err(Error::InvalidThickness {
            value: format!("{}", manifest.thickness_mm),
        });
    }

    Ok(manifest)
}

#[derive(Debug, Clone)]
struct Tag {
    val_line: usize,
    code: i32,
    value: String,
}

#[derive(Debug)]
struct RawEntity {
    kind: String,
    layer: String,
    tags: Vec<Tag>,
    vertices: Vec<Vec<Tag>>,
}

fn parse_dxf_entities(dxf_str: &str) -> Result<Vec<RawEntity>, Error> {
    let lines: Vec<&str> = dxf_str.lines().collect();
    if !lines.len().is_multiple_of(2) {
        return Err(Error::DxfOddLineCount);
    }

    let mut pairs: Vec<Tag> = Vec::with_capacity(lines.len() / 2);
    let mut saw_eof = false;
    for chunk_idx in 0..(lines.len() / 2) {
        let code_line = 2 * chunk_idx + 1;
        let val_line = 2 * chunk_idx + 2;
        let code_text = lines[2 * chunk_idx].trim();
        let code: i32 = code_text.parse().map_err(|_| Error::DxfInvalidGroupCode {
            line: code_line,
            text: code_text.to_string(),
        })?;
        let value = lines[2 * chunk_idx + 1].trim().to_string();
        if code == 0 && value == "EOF" {
            saw_eof = true;
            pairs.push(Tag {
                val_line,
                code,
                value,
            });
            break;
        }
        pairs.push(Tag {
            val_line,
            code,
            value,
        });
    }

    if !saw_eof {
        return Err(Error::DxfMissingEntities);
    }

    // Locate SECTION -> 2 ENTITIES ... 0 ENDSEC.
    let mut i = 0usize;
    let mut entities_slice: Option<&[Tag]> = None;
    while i < pairs.len() {
        if pairs[i].code == 0
            && pairs[i].value == "SECTION"
            && i + 1 < pairs.len()
            && pairs[i + 1].code == 2
            && pairs[i + 1].value == "ENTITIES"
        {
            let start = i + 2;
            let mut end = None;
            let mut j = start;
            while j < pairs.len() {
                if pairs[j].code == 0 && pairs[j].value == "ENDSEC" {
                    end = Some(j);
                    break;
                }
                if pairs[j].code == 0 && pairs[j].value == "EOF" {
                    break;
                }
                j += 1;
            }
            let end_idx = end.ok_or(Error::DxfMissingEntities)?;
            entities_slice = Some(&pairs[start..end_idx]);
            break;
        }
        i += 1;
    }

    let ent_tags = entities_slice.ok_or(Error::DxfMissingEntities)?;

    // Split ENTITIES section on code == 0 boundaries.
    let mut records: Vec<(String, Vec<Tag>)> = Vec::new();
    for tag in ent_tags {
        if tag.code == 0 {
            records.push((tag.value.clone(), Vec::new()));
        } else if let Some(last) = records.last_mut() {
            last.1.push(tag.clone());
        } else {
            return Err(Error::DxfMissingEntities);
        }
    }

    let mut out = Vec::new();
    let mut r_idx = 0usize;
    while r_idx < records.len() {
        let (ref kind, ref tags) = records[r_idx];
        if kind == "VERTEX" || kind == "SEQEND" {
            return Err(Error::DxfUnclosedPolyline);
        }
        let layer = tags
            .iter()
            .find(|t| t.code == 8)
            .map(|t| t.value.clone())
            .unwrap_or_else(|| "0".to_string());

        if kind == "POLYLINE" {
            let mut vertices = Vec::new();
            r_idx += 1;
            let mut closed_seq = false;
            while r_idx < records.len() {
                let (ref sub_kind, ref sub_tags) = records[r_idx];
                if sub_kind == "VERTEX" {
                    vertices.push(sub_tags.clone());
                    r_idx += 1;
                } else if sub_kind == "SEQEND" {
                    closed_seq = true;
                    r_idx += 1;
                    break;
                } else {
                    return Err(Error::DxfUnclosedPolyline);
                }
            }
            if !closed_seq {
                return Err(Error::DxfUnclosedPolyline);
            }
            out.push(RawEntity {
                kind: kind.clone(),
                layer,
                tags: tags.clone(),
                vertices,
            });
        } else {
            out.push(RawEntity {
                kind: kind.clone(),
                layer,
                tags: tags.clone(),
                vertices: Vec::new(),
            });
            r_idx += 1;
        }
    }

    Ok(out)
}

#[derive(Debug, Default)]
struct ParsedXdata {
    id: Option<String>,
    loop_idx: Option<usize>,
    start_x_pm: Option<i128>,
    start_y_pm: Option<i128>,
    end_x_pm: Option<i128>,
    end_y_pm: Option<i128>,
    angle_increasing: Option<bool>,
    plated: Option<bool>,
    clearance_nm: Option<i64>,
    side: Option<Side>,
    max_height_nm: Option<i64>,
}

fn parse_xdata(layer: &str, tags: &[Tag]) -> Result<ParsedXdata, Error> {
    let mut out = ParsedXdata::default();
    let mut active_app: Option<&str> = None;
    let mut brace_depth = 0i32;

    // Collect only tags belonging to 1001 == BOARD_INTERFACE.
    let mut app_tags: Vec<&Tag> = Vec::new();
    for tag in tags {
        if tag.code == 1001 {
            active_app = Some(tag.value.as_str());
            continue;
        }
        if tag.code >= 1000 && active_app == Some(APP_ID) {
            app_tags.push(tag);
        }
    }

    let mut i = 0usize;
    while i < app_tags.len() {
        let t = app_tags[i];
        if t.code == 1002 {
            match t.value.as_str() {
                "{" => brace_depth += 1,
                "}" => {
                    brace_depth -= 1;
                    if brace_depth < 0 {
                        return Err(Error::InvalidXdata {
                            layer: layer.to_string(),
                            detail: "unmatched closing `}` in XDATA".to_string(),
                        });
                    }
                }
                other => {
                    return Err(Error::InvalidXdata {
                        layer: layer.to_string(),
                        detail: format!("invalid 1002 control string `{other}`"),
                    });
                }
            }
            i += 1;
            continue;
        }

        if t.code != 1000 {
            return Err(Error::InvalidXdata {
                layer: layer.to_string(),
                detail: format!("expected 1000 key tag, found group {}", t.code),
            });
        }

        let key = t.value.as_str();
        let val_tag = app_tags
            .get(i + 1)
            .copied()
            .ok_or_else(|| Error::InvalidXdata {
                layer: layer.to_string(),
                detail: format!("missing value after XDATA key `{key}`"),
            })?;
        i += 2;

        match key {
            "id" => {
                if val_tag.code != 1000 || val_tag.value.is_empty() {
                    return Err(Error::InvalidXdata {
                        layer: layer.to_string(),
                        detail: "`id` requires a non-empty 1000 string".to_string(),
                    });
                }
                out.id = Some(val_tag.value.clone());
            }
            "loop" => {
                let v = parse_xdata_int(layer, "loop", val_tag)?;
                if v < 0 {
                    return Err(Error::InvalidXdata {
                        layer: layer.to_string(),
                        detail: format!("`loop` index must be >= 0, got {v}"),
                    });
                }
                out.loop_idx = Some(v as usize);
            }
            "start_x" => {
                out.start_x_pm = Some(parse_xdata_mm_to_pm(layer, "start_x", val_tag)?);
            }
            "start_y" => {
                out.start_y_pm = Some(parse_xdata_mm_to_pm(layer, "start_y", val_tag)?);
            }
            "end_x" => {
                out.end_x_pm = Some(parse_xdata_mm_to_pm(layer, "end_x", val_tag)?);
            }
            "end_y" => {
                out.end_y_pm = Some(parse_xdata_mm_to_pm(layer, "end_y", val_tag)?);
            }
            "angle_increasing" => {
                let v = parse_xdata_int(layer, "angle_increasing", val_tag)?;
                out.angle_increasing = match v {
                    0 => Some(false),
                    1 => Some(true),
                    _ => {
                        return Err(Error::InvalidXdata {
                            layer: layer.to_string(),
                            detail: format!("`angle_increasing` must be 0 or 1, got {v}"),
                        });
                    }
                };
            }
            "plated" => {
                let v = parse_xdata_int(layer, "plated", val_tag)?;
                out.plated = match v {
                    0 => Some(false),
                    1 => Some(true),
                    _ => {
                        return Err(Error::InvalidXdata {
                            layer: layer.to_string(),
                            detail: format!("`plated` must be 0 or 1, got {v}"),
                        });
                    }
                };
            }
            "clearance_mm" => {
                let pm = parse_xdata_mm_to_pm(layer, "clearance_mm", val_tag)?;
                let nm = pm_to_nm_checked(pm).ok_or_else(|| Error::InvalidXdata {
                    layer: layer.to_string(),
                    detail: "`clearance_mm` out of i64 nanometre range".to_string(),
                })?;
                if nm < 0 {
                    return Err(Error::InvalidXdata {
                        layer: layer.to_string(),
                        detail: format!("`clearance_mm` must be >= 0, got {nm} nm"),
                    });
                }
                out.clearance_nm = Some(nm);
            }
            "side" => {
                if val_tag.code != 1000 {
                    return Err(Error::InvalidXdata {
                        layer: layer.to_string(),
                        detail: "`side` requires a 1000 string".to_string(),
                    });
                }
                let side = Side::parse(&val_tag.value).ok_or_else(|| Error::InvalidXdata {
                    layer: layer.to_string(),
                    detail: format!(
                        "invalid keepout `side` `{}` (expected both|front|back)",
                        val_tag.value
                    ),
                })?;
                out.side = Some(side);
            }
            "max_height_mm" => {
                let pm = parse_xdata_mm_to_pm(layer, "max_height_mm", val_tag)?;
                let nm = pm_to_nm_checked(pm).ok_or_else(|| Error::InvalidXdata {
                    layer: layer.to_string(),
                    detail: "`max_height_mm` out of i64 nanometre range".to_string(),
                })?;
                if nm < 0 {
                    return Err(Error::InvalidXdata {
                        layer: layer.to_string(),
                        detail: format!("`max_height_mm` must be >= 0, got {nm} nm"),
                    });
                }
                out.max_height_nm = Some(nm);
            }
            unknown => {
                return Err(Error::InvalidXdata {
                    layer: layer.to_string(),
                    detail: format!("unknown BOARD_INTERFACE XDATA key `{unknown}`"),
                });
            }
        }
    }

    if brace_depth != 0 {
        return Err(Error::InvalidXdata {
            layer: layer.to_string(),
            detail: "unclosed `{` in XDATA".to_string(),
        });
    }

    let arc_coord_count = [
        out.start_x_pm.is_some(),
        out.start_y_pm.is_some(),
        out.end_x_pm.is_some(),
        out.end_y_pm.is_some(),
    ]
    .into_iter()
    .filter(|b| *b)
    .count();
    if arc_coord_count != 0 && arc_coord_count != 4 {
        return Err(Error::InvalidXdata {
            layer: layer.to_string(),
            detail: "incomplete arc endpoint coordinates in XDATA".to_string(),
        });
    }

    Ok(out)
}

fn validate_hole_xdata(layer: &str, xdata: &ParsedXdata) -> Result<(), Error> {
    if xdata.loop_idx.is_some()
        || xdata.start_x_pm.is_some()
        || xdata.angle_increasing.is_some()
        || xdata.side.is_some()
        || xdata.max_height_nm.is_some()
    {
        return Err(Error::InvalidXdata {
            layer: layer.to_string(),
            detail: "non-hole attribute attached to BOARD_HOLE entity".to_string(),
        });
    }
    Ok(())
}

fn parse_xdata_int(layer: &str, key: &str, tag: &Tag) -> Result<i64, Error> {
    if tag.code != 1070 && tag.code != 1071 {
        return Err(Error::InvalidXdata {
            layer: layer.to_string(),
            detail: format!(
                "`{key}` expects integer group 1070 or 1071, got {}",
                tag.code
            ),
        });
    }
    tag.value.parse::<i64>().map_err(|_| Error::InvalidXdata {
        layer: layer.to_string(),
        detail: format!("invalid integer `{}` for `{key}`", tag.value),
    })
}

fn parse_xdata_mm_to_pm(layer: &str, key: &str, tag: &Tag) -> Result<i128, Error> {
    if tag.code != 1040 {
        return Err(Error::InvalidXdata {
            layer: layer.to_string(),
            detail: format!("`{key}` expects real group 1040, got {}", tag.code),
        });
    }
    parse_mm_to_pm(&tag.value).ok_or_else(|| Error::InvalidXdata {
        layer: layer.to_string(),
        detail: format!("invalid real `{}` for `{key}`", tag.value),
    })
}

#[derive(Debug, Clone)]
struct ParsedSeg {
    element: Element,
    /// `true` when arc endpoints were reconstructed from DXF angles (no XDATA), allowing `100 nm`
    /// endpoint welding during loop chaining.
    approx_endpoints: bool,
}

#[derive(Debug, Clone)]
struct LoopItem {
    seg: ParsedSeg,
    loop_idx: Option<usize>,
}

#[derive(Debug, Clone)]
struct KeepoutItem {
    seg: ParsedSeg,
    loop_idx: Option<usize>,
    side: Option<Side>,
    max_height_nm: Option<i64>,
}

fn parse_loop_entity(raw: &RawEntity, xdata: &ParsedXdata) -> Result<Vec<ParsedSeg>, Error> {
    let layer = raw.layer.as_str();
    if raw.kind != "ARC" && (xdata.start_x_pm.is_some() || xdata.angle_increasing.is_some()) {
        return Err(Error::InvalidXdata {
            layer: raw.layer.clone(),
            detail: format!("arc endpoint XDATA attached to `{}` entity", raw.kind),
        });
    }

    match raw.kind.as_str() {
        "LINE" => {
            let x1 = require_coord_nm(&raw.tags, 10, false, layer)?;
            let y1 = require_coord_nm(&raw.tags, 20, true, layer)?;
            let x2 = require_coord_nm(&raw.tags, 11, false, layer)?;
            let y2 = require_coord_nm(&raw.tags, 21, true, layer)?;
            let start = Point::new(x1, y1);
            let end = Point::new(x2, y2);
            if start == end {
                return Err(Error::DegenerateGeometry {
                    layer: raw.layer.clone(),
                    detail: "zero-length LINE".to_string(),
                });
            }
            Ok(vec![ParsedSeg {
                element: Element::new(xdata.id.clone(), ElementKind::Line { start, end }),
                approx_endpoints: false,
            }])
        }
        "CIRCLE" => {
            let (center, diameter_nm) = parse_circle_geom(layer, &raw.tags)?;
            Ok(vec![ParsedSeg {
                element: Element::new(
                    xdata.id.clone(),
                    ElementKind::Circle {
                        center,
                        diameter_nm,
                    },
                ),
                approx_endpoints: false,
            }])
        }
        "ARC" => {
            let seg = parse_arc_geom(layer, &raw.tags, xdata)?;
            Ok(vec![seg])
        }
        "LWPOLYLINE" => parse_lwpolyline_geom(layer, &raw.tags, xdata),
        "POLYLINE" => parse_polyline_geom(layer, &raw.tags, &raw.vertices, xdata),
        other => Err(Error::UnsupportedEntity {
            layer: raw.layer.clone(),
            kind: other.to_string(),
        }),
    }
}

fn parse_circle_geom(layer: &str, tags: &[Tag]) -> Result<(Point, i64), Error> {
    let cx = require_coord_nm(tags, 10, false, layer)?;
    let cy = require_coord_nm(tags, 20, true, layer)?;
    let r_pm = require_mm_pm(tags, 40, layer)?;
    if r_pm <= 0 {
        return Err(Error::DegenerateGeometry {
            layer: layer.to_string(),
            detail: "CIRCLE radius must be > 0".to_string(),
        });
    }
    let diameter_nm =
        pm_to_nm_checked(r_pm.saturating_mul(2)).ok_or_else(|| Error::DegenerateGeometry {
            layer: layer.to_string(),
            detail: "CIRCLE diameter overflows i64 nanometres".to_string(),
        })?;
    if diameter_nm <= 0 {
        return Err(Error::DegenerateGeometry {
            layer: layer.to_string(),
            detail: "CIRCLE diameter rounds to 0 nm".to_string(),
        });
    }
    Ok((Point::new(cx, cy), diameter_nm))
}

fn parse_arc_geom(layer: &str, tags: &[Tag], xdata: &ParsedXdata) -> Result<ParsedSeg, Error> {
    let cx_nm = require_coord_nm(tags, 10, false, layer)?;
    let cy_nm = require_coord_nm(tags, 20, true, layer)?;
    let center = Point::new(cx_nm, cy_nm);

    let r_pm = require_mm_pm(tags, 40, layer)?;
    if r_pm <= 0 {
        return Err(Error::DegenerateGeometry {
            layer: layer.to_string(),
            detail: "ARC radius must be > 0".to_string(),
        });
    }
    let radius_nm = (r_pm as f64) / 1_000.0;
    let a50_deg = require_f64(tags, 50, layer)?;
    let a51_deg = require_f64(tags, 51, layer)?;

    let sweep_deg = (a51_deg - a50_deg).rem_euclid(360.0);
    if !(1e-9..=360.0 - 1e-9).contains(&sweep_deg) {
        return Err(Error::DegenerateGeometry {
            layer: layer.to_string(),
            detail: "ARC has zero or full-circle sweep".to_string(),
        });
    }

    // Approximate endpoints in KiCad's Y-down frame from the DXF Y-up CCW angles 50 and 51:
    // x_ydown = cx_dxf + r*cos(a), y_ydown = -(cy_dxf + r*sin(a)) = cy_ydown - r*sin(a).
    let a50_rad = a50_deg.to_radians();
    let a51_rad = a51_deg.to_radians();
    let p50_ydown_f = [
        (cx_nm as f64) + radius_nm * a50_rad.cos(),
        (cy_nm as f64) - radius_nm * a50_rad.sin(),
    ];
    let p51_ydown_f = [
        (cx_nm as f64) + radius_nm * a51_rad.cos(),
        (cy_nm as f64) - radius_nm * a51_rad.sin(),
    ];

    if let (Some(sx_pm), Some(sy_pm), Some(ex_pm), Some(ey_pm)) = (
        xdata.start_x_pm,
        xdata.start_y_pm,
        xdata.end_x_pm,
        xdata.end_y_pm,
    ) {
        let sx = pm_to_nm_checked(sx_pm).ok_or_else(|| Error::InvalidXdata {
            layer: layer.to_string(),
            detail: "arc start_x overflows i64".to_string(),
        })?;
        let sy = pm_to_nm_checked(-sy_pm).ok_or_else(|| Error::InvalidXdata {
            layer: layer.to_string(),
            detail: "arc start_y overflows i64".to_string(),
        })?;
        let ex = pm_to_nm_checked(ex_pm).ok_or_else(|| Error::InvalidXdata {
            layer: layer.to_string(),
            detail: "arc end_x overflows i64".to_string(),
        })?;
        let ey = pm_to_nm_checked(-ey_pm).ok_or_else(|| Error::InvalidXdata {
            layer: layer.to_string(),
            detail: "arc end_y overflows i64".to_string(),
        })?;

        let start = Point::new(sx, sy);
        let end = Point::new(ex, ey);
        if start == end {
            return Err(Error::DegenerateGeometry {
                layer: layer.to_string(),
                detail: "ARC start and end points are identical".to_string(),
            });
        }

        // Verify start and end are concentric with `center`.
        let r_start = ((start.x - center.x) as f64).hypot((start.y - center.y) as f64);
        let r_end = ((end.x - center.x) as f64).hypot((end.y - center.y) as f64);
        let r_tol = 10.0_f64.max(radius_nm * 1e-6);
        if (r_start - r_end).abs() > r_tol || (r_start - radius_nm).abs() > 10_000.0_f64.max(r_tol)
        {
            return Err(Error::DegenerateGeometry {
                layer: layer.to_string(),
                detail: "ARC start/end radii do not match center and radius".to_string(),
            });
        }

        let err_fwd = dist_point_f64(start, p50_ydown_f) + dist_point_f64(end, p51_ydown_f);
        let err_rev = dist_point_f64(start, p51_ydown_f) + dist_point_f64(end, p50_ydown_f);

        let angle_increasing = match xdata.angle_increasing {
            Some(flag) => flag,
            None => err_rev < err_fwd,
        };

        let expected_err = if angle_increasing { err_rev } else { err_fwd };
        let endpoint_tol = 20_000.0_f64.max(radius_nm * 1e-5);
        if expected_err > endpoint_tol {
            return Err(Error::InvalidXdata {
                layer: layer.to_string(),
                detail: format!(
                    "ARC XDATA endpoints disagree with DXF ARC angles/radius by {expected_err:.1} nm"
                ),
            });
        }

        Ok(ParsedSeg {
            element: Element::new(
                xdata.id.clone(),
                ElementKind::Arc {
                    center,
                    start,
                    end,
                    angle_increasing,
                },
            ),
            approx_endpoints: false,
        })
    } else {
        // Re-saved DXF without XDATA: reconstruct endpoints from angles 50/51.
        // In DXF Y-up, 50 -> 51 sweeps CCW, which flips to angle-decreasing (`angle_increasing = false`) in Y-down.
        let sx = f64_to_i64_checked(p50_ydown_f[0].round()).ok_or_else(|| {
            Error::DegenerateGeometry {
                layer: layer.to_string(),
                detail: "ARC start_x overflows i64".to_string(),
            }
        })?;
        let sy = f64_to_i64_checked(p50_ydown_f[1].round()).ok_or_else(|| {
            Error::DegenerateGeometry {
                layer: layer.to_string(),
                detail: "ARC start_y overflows i64".to_string(),
            }
        })?;
        let ex = f64_to_i64_checked(p51_ydown_f[0].round()).ok_or_else(|| {
            Error::DegenerateGeometry {
                layer: layer.to_string(),
                detail: "ARC end_x overflows i64".to_string(),
            }
        })?;
        let ey = f64_to_i64_checked(p51_ydown_f[1].round()).ok_or_else(|| {
            Error::DegenerateGeometry {
                layer: layer.to_string(),
                detail: "ARC end_y overflows i64".to_string(),
            }
        })?;
        let start = Point::new(sx, sy);
        let end = Point::new(ex, ey);
        if start == end {
            return Err(Error::DegenerateGeometry {
                layer: layer.to_string(),
                detail: "reconstructed ARC endpoints collapsed to one point".to_string(),
            });
        }

        Ok(ParsedSeg {
            element: Element::new(
                xdata.id.clone(),
                ElementKind::Arc {
                    center,
                    start,
                    end,
                    angle_increasing: false,
                },
            ),
            approx_endpoints: true,
        })
    }
}

#[derive(Debug, Clone, Copy)]
struct PolyVert {
    x_dxf_pm: i128,
    y_dxf_pm: i128,
    bulge: f64,
}

fn parse_lwpolyline_geom(
    layer: &str,
    tags: &[Tag],
    xdata: &ParsedXdata,
) -> Result<Vec<ParsedSeg>, Error> {
    let flags = optional_i64(tags, 70)?.unwrap_or(0);
    let closed = (flags & 1) != 0;

    let mut verts: Vec<PolyVert> = Vec::new();
    let mut pending_x: Option<i128> = None;
    for t in tags {
        match t.code {
            10 => {
                if pending_x.is_some() {
                    return Err(Error::DegenerateGeometry {
                        layer: layer.to_string(),
                        detail: "LWPOLYLINE vertex has group 10 without group 20".to_string(),
                    });
                }
                let x_pm = parse_mm_to_pm(&t.value).ok_or_else(|| Error::DxfInvalidValue {
                    line: t.val_line,
                    code: t.code,
                    text: t.value.clone(),
                })?;
                pending_x = Some(x_pm);
            }
            20 => {
                let x_pm = pending_x.take().ok_or_else(|| Error::DegenerateGeometry {
                    layer: layer.to_string(),
                    detail: "LWPOLYLINE has group 20 before group 10".to_string(),
                })?;
                let y_pm = parse_mm_to_pm(&t.value).ok_or_else(|| Error::DxfInvalidValue {
                    line: t.val_line,
                    code: t.code,
                    text: t.value.clone(),
                })?;
                verts.push(PolyVert {
                    x_dxf_pm: x_pm,
                    y_dxf_pm: y_pm,
                    bulge: 0.0,
                });
            }
            42 => {
                let b = parse_finite_f64(&t.value).ok_or_else(|| Error::DxfInvalidValue {
                    line: t.val_line,
                    code: t.code,
                    text: t.value.clone(),
                })?;
                if let Some(last) = verts.last_mut() {
                    last.bulge = b;
                }
            }
            _ => {}
        }
    }
    if pending_x.is_some() {
        return Err(Error::DegenerateGeometry {
            layer: layer.to_string(),
            detail: "LWPOLYLINE vertex missing group 20".to_string(),
        });
    }

    expand_polyline_vertices(layer, &verts, closed, xdata)
}

fn parse_polyline_geom(
    layer: &str,
    poly_tags: &[Tag],
    vertex_records: &[Vec<Tag>],
    xdata: &ParsedXdata,
) -> Result<Vec<ParsedSeg>, Error> {
    let flags = optional_i64(poly_tags, 70)?.unwrap_or(0);
    // Reject 3D mesh (16) and polyface mesh (64) POLYLINE variants.
    if (flags & (16 | 64)) != 0 {
        return Err(Error::UnsupportedEntity {
            layer: layer.to_string(),
            kind: "POLYLINE(MESH)".to_string(),
        });
    }
    let closed = (flags & 1) != 0;

    let mut verts = Vec::with_capacity(vertex_records.len());
    for v_tags in vertex_records {
        let x_pm = require_mm_pm(v_tags, 10, layer)?;
        let y_pm = require_mm_pm(v_tags, 20, layer)?;
        let bulge = match v_tags.iter().find(|t| t.code == 42) {
            Some(bt) => parse_finite_f64(&bt.value).ok_or_else(|| Error::DxfInvalidValue {
                line: bt.val_line,
                code: bt.code,
                text: bt.value.clone(),
            })?,
            None => 0.0,
        };
        verts.push(PolyVert {
            x_dxf_pm: x_pm,
            y_dxf_pm: y_pm,
            bulge,
        });
    }

    expand_polyline_vertices(layer, &verts, closed, xdata)
}

fn expand_polyline_vertices(
    layer: &str,
    verts: &[PolyVert],
    closed: bool,
    xdata: &ParsedXdata,
) -> Result<Vec<ParsedSeg>, Error> {
    if verts.len() < 2 {
        return Err(Error::DegenerateGeometry {
            layer: layer.to_string(),
            detail: "polyline has fewer than 2 vertices".to_string(),
        });
    }

    let pts_ydown: Vec<Point> = verts
        .iter()
        .map(|v| {
            let x = pm_to_nm_checked(v.x_dxf_pm).ok_or_else(|| Error::DegenerateGeometry {
                layer: layer.to_string(),
                detail: "polyline vertex x overflows i64".to_string(),
            })?;
            let y = pm_to_nm_checked(-v.y_dxf_pm).ok_or_else(|| Error::DegenerateGeometry {
                layer: layer.to_string(),
                detail: "polyline vertex y overflows i64".to_string(),
            })?;
            Ok(Point::new(x, y))
        })
        .collect::<Result<Vec<_>, Error>>()?;

    let seg_count = if closed { verts.len() } else { verts.len() - 1 };

    // Special case: a 2-vertex closed polyline with two opposite semicircles (`|bulge| == 1`) is
    // valid, whereas a 2-vertex closed polyline with straight segments is a zero-area spike unless
    // bulged. Check distinct points first.
    let mut out = Vec::new();
    for i in 0..seg_count {
        let next = (i + 1) % verts.len();
        let start = pts_ydown[i];
        let end = pts_ydown[next];
        if start == end {
            // Skip duplicate adjacent vertices (common when closed flag AND repeated start vertex).
            continue;
        }
        let bulge = verts[i].bulge;
        // When a polyline expands to multiple segments, don't duplicate a single entity XDATA id;
        // if a single-segment polyline had an id, keep it.
        let kind = if bulge.abs() < 1e-12 {
            ElementKind::Line { start, end }
        } else {
            bulge_to_arc_ydown(layer, verts[i], verts[next], start, end, bulge)?
        };
        out.push(ParsedSeg {
            element: Element::new(None, kind),
            approx_endpoints: false,
        });
    }

    if out.is_empty() {
        return Err(Error::DegenerateGeometry {
            layer: layer.to_string(),
            detail: "polyline has fewer than 2 distinct vertices".to_string(),
        });
    }
    if out.len() == 1 {
        out[0].element.id = xdata.id.clone();
    } else if let Some(base_id) = &xdata.id {
        for (idx, seg) in out.iter_mut().enumerate() {
            seg.element.id = Some(format!("{base_id}:{idx}"));
        }
    }

    // A 2-segment closed loop of two collinear lines (`A -> B` and `B -> A`) is degenerate.
    if closed
        && out.len() == 2
        && matches!(out[0].element.kind, ElementKind::Line { .. })
        && matches!(out[1].element.kind, ElementKind::Line { .. })
    {
        return Err(Error::DegenerateGeometry {
            layer: layer.to_string(),
            detail: "closed 2-vertex straight polyline has zero area".to_string(),
        });
    }

    Ok(out)
}

/// Convert a DXF bulged segment (`bulge = tan(theta / 4)` in DXF's Y-up frame) into a Y-down `Arc`.
fn bulge_to_arc_ydown(
    layer: &str,
    a: PolyVert,
    b: PolyVert,
    start_ydown: Point,
    end_ydown: Point,
    bulge: f64,
) -> Result<ElementKind, Error> {
    let ax_nm = (a.x_dxf_pm as f64) / 1_000.0;
    let ay_nm = (a.y_dxf_pm as f64) / 1_000.0;
    let bx_nm = (b.x_dxf_pm as f64) / 1_000.0;
    let by_nm = (b.y_dxf_pm as f64) / 1_000.0;

    let dx = bx_nm - ax_nm;
    let dy = by_nm - ay_nm;
    // Exact rational center formula from `cot(theta / 2) = (1 - bulge^2) / (2 * bulge)`:
    // c = mid + (-dy, dx) * (1 - bulge^2) / (4 * bulge).
    let k = (1.0 - bulge * bulge) / (4.0 * bulge);
    let cx_dxf_nm = 0.5 * (ax_nm + bx_nm) - dy * k;
    let cy_dxf_nm = 0.5 * (ay_nm + by_nm) + dx * k;

    let cx = f64_to_i64_checked(cx_dxf_nm.round()).ok_or_else(|| Error::DegenerateGeometry {
        layer: layer.to_string(),
        detail: "bulge arc center x overflows i64".to_string(),
    })?;
    let cy = f64_to_i64_checked((-cy_dxf_nm).round()).ok_or_else(|| Error::DegenerateGeometry {
        layer: layer.to_string(),
        detail: "bulge arc center y overflows i64".to_string(),
    })?;

    // `bulge > 0` sweeps CCW in DXF Y-up, which flips to `angle_increasing = false` in KiCad Y-down.
    // `bulge < 0` sweeps CW in DXF Y-up, which flips to `angle_increasing = true` in KiCad Y-down.
    Ok(ElementKind::Arc {
        center: Point::new(cx, cy),
        start: start_ydown,
        end: end_ydown,
        angle_increasing: bulge < 0.0,
    })
}

fn build_cutout_loops(items: Vec<LoopItem>) -> Result<Vec<Loop>, Error> {
    if items.is_empty() {
        return Ok(Vec::new());
    }
    let mut by_idx: BTreeMap<usize, Vec<ParsedSeg>> = BTreeMap::new();
    let mut unindexed: Vec<ParsedSeg> = Vec::new();
    for item in items {
        match item.loop_idx {
            Some(k) => by_idx.entry(k).or_default().push(item.seg),
            None => unindexed.push(item.seg),
        }
    }
    let mut loops = Vec::new();
    for segs in by_idx.into_values() {
        loops.extend(chain_segments(LAYER_CUTOUT, segs)?);
    }
    if !unindexed.is_empty() {
        loops.extend(chain_segments(LAYER_CUTOUT, unindexed)?);
    }
    Ok(loops)
}

fn build_keepouts(items: Vec<KeepoutItem>) -> Result<Vec<Keepout>, Error> {
    if items.is_empty() {
        return Ok(Vec::new());
    }
    let mut by_idx: BTreeMap<usize, Vec<KeepoutItem>> = BTreeMap::new();
    let mut unindexed: Vec<ParsedSeg> = Vec::new();
    for item in items {
        match item.loop_idx {
            Some(k) => by_idx.entry(k).or_default().push(item),
            None => unindexed.push(item.seg),
        }
    }

    let mut out = Vec::new();
    for (loop_index, group) in by_idx {
        let first_side = group[0].side;
        let first_height = group[0].max_height_nm;
        for g in &group[1..] {
            if g.side != first_side || g.max_height_nm != first_height {
                return Err(Error::InconsistentKeepoutAttributes { loop_index });
            }
        }
        let segs: Vec<ParsedSeg> = group.into_iter().map(|g| g.seg).collect();
        let loops = chain_segments(LAYER_KEEPOUT, segs)?;
        for boundary in loops {
            out.push(Keepout {
                boundary,
                side: first_side.unwrap_or(Side::Both),
                max_height_nm: first_height,
            });
        }
    }

    if !unindexed.is_empty() {
        let loops = chain_segments(LAYER_KEEPOUT, unindexed)?;
        for boundary in loops {
            out.push(Keepout {
                boundary,
                side: Side::Both,
                max_height_nm: None,
            });
        }
    }

    Ok(out)
}

/// Chain `segs` into closed loops on `layer`.
fn chain_segments(layer: &str, segs: Vec<ParsedSeg>) -> Result<Vec<Loop>, Error> {
    let mut loops = Vec::new();
    let mut opens: Vec<ParsedSeg> = Vec::new();
    let any_approx = segs.iter().any(|s| s.approx_endpoints);

    for seg in segs {
        match seg.element.kind {
            ElementKind::Circle { .. } => loops.push(Loop::new(vec![seg.element])),
            ElementKind::Line { .. } | ElementKind::Arc { .. } => opens.push(seg),
        }
    }

    if opens.is_empty() {
        return Ok(loops);
    }

    // Build vertex table: exact 0 nm equality when XDATA is present; 100 nm weld when XDATA was
    // stripped by a re-save (preferring exact Line endpoints as canonical node positions).
    let mut nodes: Vec<Point> = Vec::new();
    if any_approx {
        // Seed exact (non-approx) endpoints first so reconstructed arc ends snap onto exact line ends.
        for o in &opens {
            if !o.approx_endpoints
                && let Some((a, b)) = o.element.kind.endpoints()
            {
                for p in [a, b] {
                    if !nodes.iter().any(|q| dist_points(*q, p) <= RESAVE_WELD_NM) {
                        nodes.push(p);
                    }
                }
            }
        }
    }

    let mut node_of = |p: Point| -> usize {
        if any_approx {
            if let Some(idx) = nodes
                .iter()
                .position(|q| dist_points(*q, p) <= RESAVE_WELD_NM)
            {
                return idx;
            }
        } else if let Some(idx) = nodes.iter().position(|q| *q == p) {
            return idx;
        }
        nodes.push(p);
        nodes.len() - 1
    };

    let mut ends: Vec<(usize, usize)> = Vec::with_capacity(opens.len());
    for o in &opens {
        let (a, b) = o.element.kind.endpoints().expect("open segment");
        let na = node_of(a);
        let nb = node_of(b);
        if na == nb {
            return Err(Error::DegenerateGeometry {
                layer: layer.to_string(),
                detail: "segment endpoints welded to the same vertex".to_string(),
            });
        }
        ends.push((na, nb));
    }

    let mut degree = vec![0usize; nodes.len()];
    let mut incident: Vec<Vec<usize>> = vec![Vec::new(); nodes.len()];
    for (seg_idx, (na, nb)) in ends.iter().enumerate() {
        degree[*na] += 1;
        degree[*nb] += 1;
        incident[*na].push(seg_idx);
        incident[*nb].push(seg_idx);
    }

    for (node_idx, &deg) in degree.iter().enumerate() {
        if deg == 1 {
            let at = nodes[node_idx];
            let gap = nodes
                .iter()
                .enumerate()
                .filter(|(j, _)| *j != node_idx)
                .map(|(_, q)| dist_points(*q, at).round() as i64)
                .min()
                .unwrap_or(0);
            return Err(Error::OpenLoop {
                layer: layer.to_string(),
                at,
                gap_nm: gap,
            });
        }
        if deg > 2 {
            return Err(Error::BranchingLoop {
                layer: layer.to_string(),
                at: nodes[node_idx],
                degree: deg,
            });
        }
    }

    let mut used = vec![false; opens.len()];
    for start_idx in 0..opens.len() {
        if used[start_idx] {
            continue;
        }
        let mut loop_elems = Vec::new();
        let mut cur = start_idx;
        let mut cur_from = ends[start_idx].0;
        loop {
            used[cur] = true;
            let (na, nb) = ends[cur];
            let forward = na == cur_from;
            let next_node = if forward { nb } else { na };
            let mut el = if forward {
                opens[cur].element.clone()
            } else {
                opens[cur].element.reversed()
            };
            // Snap welded endpoints to canonical node coordinates so chained segments match to 0 nm.
            el = snap_endpoints(el, nodes[cur_from], nodes[next_node]);
            loop_elems.push(el);

            match incident[next_node].iter().copied().find(|&s| !used[s]) {
                Some(next_seg) => {
                    cur = next_seg;
                    cur_from = next_node;
                }
                None => break,
            }
        }

        // Reject 2-line zero-area loops (`A -> B` and `B -> A`).
        if loop_elems.len() == 2
            && matches!(loop_elems[0].kind, ElementKind::Line { .. })
            && matches!(loop_elems[1].kind, ElementKind::Line { .. })
        {
            return Err(Error::DegenerateGeometry {
                layer: layer.to_string(),
                detail: "two collinear lines do not bound an area".to_string(),
            });
        }

        loops.push(Loop::new(loop_elems));
    }

    Ok(loops)
}

fn snap_endpoints(mut el: Element, start_pt: Point, end_pt: Point) -> Element {
    match &mut el.kind {
        ElementKind::Line { start, end } => {
            *start = start_pt;
            *end = end_pt;
        }
        ElementKind::Arc { start, end, .. } => {
            *start = start_pt;
            *end = end_pt;
        }
        ElementKind::Circle { .. } => {}
    }
    el
}

fn validate_cutouts_inside_outline(outline: &Loop, cutouts: &[Loop]) -> Result<(), Error> {
    if cutouts.is_empty() {
        return Ok(());
    }
    let outer_poly = sample_loop(outline);
    let cutout_polys: Vec<Vec<[f64; 2]>> = cutouts.iter().map(sample_loop).collect();

    for (i, poly) in cutout_polys.iter().enumerate() {
        // Every sampled boundary point of the cutout must lie strictly inside `outer_poly`.
        for &pt in poly {
            if !point_in_polygon(pt, &outer_poly) {
                return Err(Error::CutoutOutsideOutline);
            }
        }
        // Cutouts must not be nested inside another cutout.
        let probe = poly[0];
        for (j, other_poly) in cutout_polys.iter().enumerate() {
            if i != j && point_in_polygon(probe, other_poly) {
                return Err(Error::CutoutOutsideOutline);
            }
        }
    }
    Ok(())
}

fn sample_loop(lp: &Loop) -> Vec<[f64; 2]> {
    let mut pts = Vec::new();
    for el in &lp.elements {
        match el.kind {
            ElementKind::Line { start, .. } => pts.push([start.x as f64, start.y as f64]),
            ElementKind::Arc {
                center,
                start,
                end,
                angle_increasing,
            } => {
                let r = ((start.x - center.x) as f64).hypot((start.y - center.y) as f64);
                let a0 = angle_ydown(center, start);
                let th = sweep_ydown(center, start, end, angle_increasing);
                let steps = ((th / (std::f64::consts::TAU / 256.0)).ceil() as usize).max(16);
                let dir = if angle_increasing { 1.0 } else { -1.0 };
                for k in 0..steps {
                    let t = a0 + dir * th * (k as f64 / steps as f64);
                    pts.push([
                        (center.x as f64) + r * t.cos(),
                        (center.y as f64) + r * t.sin(),
                    ]);
                }
            }
            ElementKind::Circle {
                center,
                diameter_nm,
            } => {
                let r = (diameter_nm as f64) * 0.5;
                let steps = 256usize;
                for k in 0..steps {
                    let t = std::f64::consts::TAU * (k as f64 / steps as f64);
                    pts.push([
                        (center.x as f64) + r * t.cos(),
                        (center.y as f64) + r * t.sin(),
                    ]);
                }
            }
        }
    }
    pts
}

fn point_in_polygon(p: [f64; 2], poly: &[[f64; 2]]) -> bool {
    if poly.len() < 3 {
        return false;
    }
    let mut inside = false;
    let mut j = poly.len() - 1;
    for i in 0..poly.len() {
        let (xi, yi) = (poly[i][0], poly[i][1]);
        let (xj, yj) = (poly[j][0], poly[j][1]);
        if (yi > p[1]) != (yj > p[1]) && p[0] < (xj - xi) * (p[1] - yi) / (yj - yi) + xi {
            inside = !inside;
        }
        j = i;
    }
    inside
}

fn require_coord_nm(tags: &[Tag], code: i32, negate: bool, layer: &str) -> Result<i64, Error> {
    let pm = require_mm_pm(tags, code, layer)?;
    let signed_pm = if negate { -pm } else { pm };
    pm_to_nm_checked(signed_pm).ok_or_else(|| Error::DegenerateGeometry {
        layer: layer.to_string(),
        detail: format!("coordinate group {code} overflows i64 nanometres"),
    })
}

fn require_mm_pm(tags: &[Tag], code: i32, layer: &str) -> Result<i128, Error> {
    let t = tags
        .iter()
        .find(|t| t.code == code)
        .ok_or_else(|| Error::DegenerateGeometry {
            layer: layer.to_string(),
            detail: format!("missing required group code {code}"),
        })?;
    parse_mm_to_pm(&t.value).ok_or_else(|| Error::DxfInvalidValue {
        line: t.val_line,
        code: t.code,
        text: t.value.clone(),
    })
}

fn require_f64(tags: &[Tag], code: i32, layer: &str) -> Result<f64, Error> {
    let t = tags
        .iter()
        .find(|t| t.code == code)
        .ok_or_else(|| Error::DegenerateGeometry {
            layer: layer.to_string(),
            detail: format!("missing required group code {code}"),
        })?;
    parse_finite_f64(&t.value).ok_or_else(|| Error::DxfInvalidValue {
        line: t.val_line,
        code: t.code,
        text: t.value.clone(),
    })
}

fn optional_i64(tags: &[Tag], code: i32) -> Result<Option<i64>, Error> {
    let Some(t) = tags.iter().find(|t| t.code == code) else {
        return Ok(None);
    };
    let v = t.value.parse::<i64>().map_err(|_| Error::DxfInvalidValue {
        line: t.val_line,
        code: t.code,
        text: t.value.clone(),
    })?;
    Ok(Some(v))
}

fn parse_finite_f64(s: &str) -> Option<f64> {
    let v: f64 = s.trim().parse().ok()?;
    v.is_finite().then_some(v)
}

/// Parse a millimetre decimal string to exact integer picometres (`1 mm = 1_000_000_000 pm`),
/// avoiding `f64` precision loss on large coordinates and odd-nanometre half-radii. Falls back to
/// `f64` only when scientific notation (`e` / `E`) is used by a third-party DXF writer.
pub(crate) fn parse_mm_to_pm(text: &str) -> Option<i128> {
    let s = text.trim();
    if s.is_empty() {
        return None;
    }
    if s.contains('e') || s.contains('E') {
        let v = parse_finite_f64(s)?;
        let pm_f = (v * 1_000_000_000.0).round();
        if !(pm_f >= (i128::MIN as f64) && pm_f < (i128::MAX as f64)) {
            return None;
        }
        return Some(pm_f as i128);
    }

    let (neg, rest) = if let Some(r) = s.strip_prefix('-') {
        (true, r)
    } else if let Some(r) = s.strip_prefix('+') {
        (false, r)
    } else {
        (false, s)
    };

    if rest.is_empty() || rest == "." {
        return None;
    }

    let (whole_str, frac_str) = match rest.split_once('.') {
        Some((w, f)) => {
            if f.contains('.') {
                return None;
            }
            (w, f)
        }
        None => (rest, ""),
    };

    if !whole_str.bytes().all(|b| b.is_ascii_digit())
        || !frac_str.bytes().all(|b| b.is_ascii_digit())
    {
        return None;
    }

    let whole: i128 = if whole_str.is_empty() {
        0
    } else {
        whole_str.parse().ok()?
    };

    let mut frac_pm: i128 = 0;
    let frac_bytes = frac_str.as_bytes();
    for i in 0..9 {
        let digit = frac_bytes.get(i).map_or(0, |b| (b - b'0') as i128);
        frac_pm = frac_pm * 10 + digit;
    }
    // Round half away from zero on the 10th fractional digit if more than 9 decimals were given.
    if frac_bytes.get(9).is_some_and(|&b| b >= b'5') {
        frac_pm += 1;
    }

    let mag = whole.checked_mul(1_000_000_000)?.checked_add(frac_pm)?;
    if neg { mag.checked_neg() } else { Some(mag) }
}

/// Round exact picometres to whole `i64` nanometres (`1 nm = 1_000 pm`, ties away from zero).
pub(crate) fn pm_to_nm_checked(pm: i128) -> Option<i64> {
    let rounded = if pm >= 0 {
        (pm + 500) / 1_000
    } else {
        (pm - 500) / 1_000
    };
    i64::try_from(rounded).ok()
}

fn f64_to_i64_checked(v: f64) -> Option<i64> {
    if !v.is_finite() || v < (i64::MIN as f64) || v >= 9_223_372_036_854_775_808.0 {
        return None;
    }
    Some(v as i64)
}

fn dist_points(a: Point, b: Point) -> f64 {
    ((a.x as f64) - (b.x as f64)).hypot((a.y as f64) - (b.y as f64))
}

fn dist_point_f64(a: Point, b: [f64; 2]) -> f64 {
    ((a.x as f64) - b[0]).hypot((a.y as f64) - b[1])
}
