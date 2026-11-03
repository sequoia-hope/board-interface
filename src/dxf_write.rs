use crate::hash::hash;
use crate::model::{
    APP_ID, Element, ElementKind, Hole, LAYER_CUTOUT, LAYER_HOLE, LAYER_KEEPOUT, LAYER_OUTLINE,
    Loop, Point, Record, Side,
};

/// Write `record` to `(board_shape_dxf, board_shape_json)`.
///
/// - DXF dialect: R12 (`AC1009`), millimetres, **Y up** (`(x, -y)` relative to `record`'s Y-down
///   frame), layers `BOARD_OUTLINE`, `BOARD_CUTOUT`, `BOARD_HOLE`, `BOARD_KEEPOUT`, entities
///   `LINE`, `ARC`, `CIRCLE`, and extended entity data under `BOARD_INTERFACE` registered in an
///   `APPID` table.
/// - Reals use nine fixed decimals, never exponents, with `-0.0` formatted as `0.000000000`.
/// - Manifest JSON has `geometry_hash` synchronized to [`hash`]`(record)` (and `revision` populated
///   with `geometry_hash` if `record.manifest.revision` was empty).
pub fn write(record: &Record) -> (String, String) {
    let mut dxf = String::new();
    let bbox = compute_extents_dxf_pm(record);
    write_header(&mut dxf, bbox);
    write_tables(&mut dxf);
    dxf.push_str("  0\nSECTION\n  2\nENTITIES\n");

    write_loop(&mut dxf, &record.outline, LAYER_OUTLINE, 0, None);
    for (idx, cutout) in record.cutouts.iter().enumerate() {
        write_loop(&mut dxf, cutout, LAYER_CUTOUT, idx, None);
    }
    for hole in &record.holes {
        write_hole(&mut dxf, hole);
    }
    for (idx, keepout) in record.keepouts.iter().enumerate() {
        write_loop(
            &mut dxf,
            &keepout.boundary,
            LAYER_KEEPOUT,
            idx,
            Some((keepout.side, keepout.max_height_nm)),
        );
    }

    dxf.push_str("  0\nENDSEC\n  0\nEOF\n");

    let geom_hash = hash(record);
    let mut manifest = record.manifest.clone();
    manifest.geometry_hash = geom_hash.clone();
    if manifest.revision.is_empty() {
        manifest.revision = geom_hash;
    }
    let mut manifest_json =
        serde_json::to_string_pretty(&manifest).expect("Manifest serialization is infallible");
    manifest_json.push('\n');

    (dxf, manifest_json)
}

fn write_header(out: &mut String, bbox_pm: Option<([i128; 2], [i128; 2])>) {
    out.push_str("  0\nSECTION\n  2\nHEADER\n");
    out.push_str("  9\n$ACADVER\n  1\nAC1009\n");
    out.push_str("  9\n$INSBASE\n");
    point_pm(out, 0, 0);
    let (min_pm, max_pm) = bbox_pm.unwrap_or(([0, 0], [0, 0]));
    out.push_str("  9\n$EXTMIN\n");
    point_pm(out, min_pm[0], min_pm[1]);
    out.push_str("  9\n$EXTMAX\n");
    point_pm(out, max_pm[0], max_pm[1]);
    out.push_str("  0\nENDSEC\n");
}

fn write_tables(out: &mut String) {
    out.push_str("  0\nSECTION\n  2\nTABLES\n");

    // Register BOARD_INTERFACE for R12 extended entity data.
    out.push_str("  0\nTABLE\n  2\nAPPID\n 70\n     1\n");
    out.push_str("  0\nAPPID\n  2\n");
    out.push_str(APP_ID);
    out.push_str("\n 70\n     0\n");
    out.push_str("  0\nENDTAB\n");

    // Declare the four standard board layers unconditionally in deterministic order.
    let layers = [
        (LAYER_OUTLINE, 7),
        (LAYER_CUTOUT, 7),
        (LAYER_HOLE, 7),
        (LAYER_KEEPOUT, 8),
    ];
    out.push_str(&format!(
        "  0\nTABLE\n  2\nLAYER\n 70\n{:6}\n",
        layers.len()
    ));
    for (name, color) in layers {
        out.push_str("  0\nLAYER\n  2\n");
        out.push_str(name);
        out.push_str("\n 70\n     0\n 62\n");
        out.push_str(&format!("{color:6}\n"));
        out.push_str("  6\nCONTINUOUS\n");
    }
    out.push_str("  0\nENDTAB\n  0\nENDSEC\n");
}

fn write_loop(
    out: &mut String,
    lp: &Loop,
    layer: &str,
    loop_idx: usize,
    keepout_meta: Option<(Side, Option<i64>)>,
) {
    for el in &lp.elements {
        write_element(out, el, layer, loop_idx, keepout_meta);
    }
}

fn write_element(
    out: &mut String,
    el: &Element,
    layer: &str,
    loop_idx: usize,
    keepout_meta: Option<(Side, Option<i64>)>,
) {
    match el.kind {
        ElementKind::Line { start, end } => {
            start_entity(out, "LINE", layer);
            point_dxf(out, start);
            second_point_dxf(out, end);
            begin_xdata(out);
            if let Some(id) = &el.id {
                xdata_str_kv(out, "id", id);
            }
            xdata_int_kv(out, "loop", loop_idx as i32);
            if let Some((side, max_h)) = keepout_meta {
                write_keepout_xdata(out, side, max_h);
            }
            end_xdata(out);
        }
        ElementKind::Arc {
            center,
            start,
            end,
            angle_increasing,
        } => {
            start_entity(out, "ARC", layer);
            point_dxf(out, center);
            let (radius_mm, a50, a51) = dxf_arc_params(center, start, end, angle_increasing);
            out.push_str(" 40\n");
            out.push_str(&format_real(radius_mm));
            out.push_str(" 50\n");
            out.push_str(&format_real(a50));
            out.push_str(" 51\n");
            out.push_str(&format_real(a51));

            begin_xdata(out);
            if let Some(id) = &el.id {
                xdata_str_kv(out, "id", id);
            }
            xdata_int_kv(out, "loop", loop_idx as i32);
            xdata_nm_kv(out, "start_x", start.x, false);
            xdata_nm_kv(out, "start_y", start.y, true);
            xdata_nm_kv(out, "end_x", end.x, false);
            xdata_nm_kv(out, "end_y", end.y, true);
            xdata_short_kv(out, "angle_increasing", i16::from(angle_increasing));
            if let Some((side, max_h)) = keepout_meta {
                write_keepout_xdata(out, side, max_h);
            }
            end_xdata(out);
        }
        ElementKind::Circle {
            center,
            diameter_nm,
        } => {
            start_entity(out, "CIRCLE", layer);
            point_dxf(out, center);
            out.push_str(" 40\n");
            out.push_str(&format_picometres((diameter_nm as i128) * 500));

            begin_xdata(out);
            if let Some(id) = &el.id {
                xdata_str_kv(out, "id", id);
            }
            xdata_int_kv(out, "loop", loop_idx as i32);
            if let Some((side, max_h)) = keepout_meta {
                write_keepout_xdata(out, side, max_h);
            }
            end_xdata(out);
        }
    }
}

fn write_hole(out: &mut String, hole: &Hole) {
    start_entity(out, "CIRCLE", LAYER_HOLE);
    point_dxf(out, hole.center);
    out.push_str(" 40\n");
    out.push_str(&format_picometres((hole.diameter_nm as i128) * 500));

    begin_xdata(out);
    if let Some(id) = &hole.id {
        xdata_str_kv(out, "id", id);
    }
    xdata_short_kv(out, "plated", i16::from(hole.plated));
    xdata_nm_kv(out, "clearance_mm", hole.clearance_nm, false);
    end_xdata(out);
}

fn write_keepout_xdata(out: &mut String, side: Side, max_height_nm: Option<i64>) {
    xdata_str_kv(out, "side", side.as_str());
    if let Some(max_h) = max_height_nm {
        xdata_nm_kv(out, "max_height_mm", max_h, false);
    }
}

/// Compute `(radius_mm, start_angle_deg_50, end_angle_deg_51)` in DXF's counter-clockwise Y-up
/// frame from a Y-down `Arc`.
pub(crate) fn dxf_arc_params(
    center: Point,
    start: Point,
    end: Point,
    ccw_ydown: bool,
) -> (f64, f64, f64) {
    let dx_s = ((start.x as i128) - (center.x as i128)) as f64 / 1_000_000.0;
    let dy_s = ((center.y as i128) - (start.y as i128)) as f64 / 1_000_000.0;
    let dx_e = ((end.x as i128) - (center.x as i128)) as f64 / 1_000_000.0;
    let dy_e = ((center.y as i128) - (end.y as i128)) as f64 / 1_000_000.0;

    let radius_mm = 0.5 * (dx_s.hypot(dy_s) + dx_e.hypot(dy_e));
    let ang_s = wrap_dxf_degrees(dy_s.atan2(dx_s).to_degrees());
    let ang_e = wrap_dxf_degrees(dy_e.atan2(dx_e).to_degrees());

    // Y flip reverses sweep: Y-down angle-increasing (`ccw_ydown = true`) becomes clockwise in
    // Y-up, so DXF's CCW arc sweeps from `end_dxf` (50) to `start_dxf` (51).
    if ccw_ydown {
        (radius_mm, ang_e, ang_s)
    } else {
        (radius_mm, ang_s, ang_e)
    }
}

pub(crate) fn wrap_dxf_degrees(deg: f64) -> f64 {
    let w = deg.rem_euclid(360.0);
    if !w.is_finite() || w >= 360.0 - 0.5e-9 {
        0.0
    } else {
        w
    }
}

fn start_entity(out: &mut String, kind: &str, layer: &str) {
    out.push_str("  0\n");
    out.push_str(kind);
    out.push_str("\n  8\n");
    out.push_str(layer);
    out.push('\n');
}

fn point_dxf(out: &mut String, p_ydown: Point) {
    point_pm(
        out,
        (p_ydown.x as i128) * 1_000,
        -(p_ydown.y as i128) * 1_000,
    );
}

fn second_point_dxf(out: &mut String, p_ydown: Point) {
    out.push_str(" 11\n");
    out.push_str(&format_picometres((p_ydown.x as i128) * 1_000));
    out.push_str(" 21\n");
    out.push_str(&format_picometres(-(p_ydown.y as i128) * 1_000));
    out.push_str(" 31\n");
    out.push_str(&format_picometres(0));
}

fn point_pm(out: &mut String, x_pm: i128, y_pm: i128) {
    out.push_str(" 10\n");
    out.push_str(&format_picometres(x_pm));
    out.push_str(" 20\n");
    out.push_str(&format_picometres(y_pm));
    out.push_str(" 30\n");
    out.push_str(&format_picometres(0));
}

fn begin_xdata(out: &mut String) {
    out.push_str("1001\n");
    out.push_str(APP_ID);
    out.push_str("\n1002\n{\n");
}

fn end_xdata(out: &mut String) {
    out.push_str("1002\n}\n");
}

fn xdata_str_kv(out: &mut String, key: &str, val: &str) {
    out.push_str("1000\n");
    out.push_str(key);
    out.push_str("\n1000\n");
    out.push_str(val);
    out.push('\n');
}

fn xdata_short_kv(out: &mut String, key: &str, val: i16) {
    out.push_str("1000\n");
    out.push_str(key);
    out.push_str(&format!("\n1070\n{val:6}\n"));
}

fn xdata_int_kv(out: &mut String, key: &str, val: i32) {
    out.push_str("1000\n");
    out.push_str(key);
    out.push_str(&format!("\n1071\n{val:6}\n"));
}

fn xdata_nm_kv(out: &mut String, key: &str, nm: i64, negate: bool) {
    let pm = if negate {
        -(nm as i128) * 1_000
    } else {
        (nm as i128) * 1_000
    };
    out.push_str("1000\n");
    out.push_str(key);
    out.push_str("\n1040\n");
    out.push_str(&format_picometres(pm));
}

/// Format an `f64` real following `dxf_export.rs`: nine fixed decimals, never an exponent, and
/// `-0.0` (including sub-0.5-picometre negatives) emitted as `0.000000000`.
pub(crate) fn format_real(v: f64) -> String {
    let v = if v == 0.0 || v.abs() < 0.5e-9 { 0.0 } else { v };
    format!("{v:.9}\n")
}

/// Format an exact integer picometre value as millimetres at nine decimal places (`1 mm = 10⁹ pm`),
/// never emitting `-0.000000000` or scientific notation.
pub(crate) fn format_picometres(pm: i128) -> String {
    if pm == 0 {
        return "0.000000000\n".to_string();
    }
    let sign = if pm < 0 { "-" } else { "" };
    let abs = pm.unsigned_abs();
    let whole = abs / 1_000_000_000;
    let frac = abs % 1_000_000_000;
    format!("{sign}{whole}.{frac:09}\n")
}

struct Extents {
    min_x: i128,
    min_y: i128,
    max_x: i128,
    max_y: i128,
}

impl Extents {
    fn new() -> Self {
        Self {
            min_x: i128::MAX,
            min_y: i128::MAX,
            max_x: i128::MIN,
            max_y: i128::MIN,
        }
    }

    fn include(&mut self, x_pm: i128, y_pm: i128) {
        self.min_x = self.min_x.min(x_pm);
        self.min_y = self.min_y.min(y_pm);
        self.max_x = self.max_x.max(x_pm);
        self.max_y = self.max_y.max(y_pm);
    }

    fn include_loop(&mut self, lp: &Loop) {
        for el in &lp.elements {
            match el.kind {
                ElementKind::Line { start, end } => {
                    self.include((start.x as i128) * 1_000, -(start.y as i128) * 1_000);
                    self.include((end.x as i128) * 1_000, -(end.y as i128) * 1_000);
                }
                ElementKind::Circle {
                    center,
                    diameter_nm,
                } => {
                    let cx = (center.x as i128) * 1_000;
                    let cy = -(center.y as i128) * 1_000;
                    let r = (diameter_nm as i128) * 500;
                    self.include(cx - r, cy - r);
                    self.include(cx + r, cy + r);
                }
                ElementKind::Arc {
                    center,
                    start,
                    end,
                    angle_increasing,
                } => {
                    self.include((start.x as i128) * 1_000, -(start.y as i128) * 1_000);
                    self.include((end.x as i128) * 1_000, -(end.y as i128) * 1_000);
                    let (r_mm, a50, a51) = dxf_arc_params(center, start, end, angle_increasing);
                    let r_pm = (r_mm * 1_000_000_000.0).round() as i128;
                    let cx = (center.x as i128) * 1_000;
                    let cy = -(center.y as i128) * 1_000;
                    for (deg, ox, oy) in [
                        (0.0, r_pm, 0i128),
                        (90.0, 0, r_pm),
                        (180.0, -r_pm, 0),
                        (270.0, 0, -r_pm),
                    ] {
                        if angle_in_ccw_arc(deg, a50, a51) {
                            self.include(cx + ox, cy + oy);
                        }
                    }
                }
            }
        }
    }
}

fn compute_extents_dxf_pm(record: &Record) -> Option<([i128; 2], [i128; 2])> {
    let mut ext = Extents::new();
    ext.include_loop(&record.outline);
    for cutout in &record.cutouts {
        ext.include_loop(cutout);
    }
    for hole in &record.holes {
        let cx = (hole.center.x as i128) * 1_000;
        let cy = -(hole.center.y as i128) * 1_000;
        let r = (hole.diameter_nm as i128) * 500;
        ext.include(cx - r, cy - r);
        ext.include(cx + r, cy + r);
    }
    for keepout in &record.keepouts {
        ext.include_loop(&keepout.boundary);
    }

    if ext.min_x <= ext.max_x {
        Some(([ext.min_x, ext.min_y], [ext.max_x, ext.max_y]))
    } else {
        None
    }
}

fn angle_in_ccw_arc(test_deg: f64, a50: f64, a51: f64) -> bool {
    if a50 <= a51 {
        test_deg >= a50 && test_deg <= a51
    } else {
        test_deg >= a50 || test_deg <= a51
    }
}
