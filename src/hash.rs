use sha2::{Digest, Sha256};

use crate::model::{Element, ElementKind, Hole, Keepout, Loop, Point, Record, Side};

/// Prefix for every geometry hash produced by [`hash`].
pub const HASH_PREFIX: &str = "sha256:";

/// Compute the canonical nanometre geometry hash of `record`.
///
/// Follows "The geometry hash" in `suite-spec.md`:
/// 1. Coordinates and diameters are hashed as whole `i64` nanometres in KiCad's Y-down frame.
/// 2. Each loop's winding direction and starting element are normalized; reversing a loop reverses
///    every segment and flips each arc's `angle_increasing` flag.
/// 3. Cutout loops, holes, and keepouts are sorted into a deterministic total order.
/// 4. Element/hole IDs, hole attributes (`plated`, `clearance_nm`), keepout attributes (`side`,
///    `max_height_nm`), and board `thickness` (in whole nanometres) are included; arc endpoints
///    are hashed directly as carried, never recomputed from angles.
pub fn hash(record: &Record) -> String {
    let outline = canonicalize_loop(&record.outline);

    let mut cutouts: Vec<Loop> = record.cutouts.iter().map(canonicalize_loop).collect();
    cutouts.sort();

    let mut holes: Vec<Hole> = record.holes.clone();
    holes.sort();

    let mut keepouts: Vec<Keepout> = record
        .keepouts
        .iter()
        .map(|k| Keepout {
            boundary: canonicalize_loop(&k.boundary),
            side: k.side,
            max_height_nm: k.max_height_nm,
        })
        .collect();
    keepouts.sort();

    let mut hasher = Sha256::new();
    hasher.update(b"board-interface:v1\0");
    hash_i64(&mut hasher, record.thickness_nm());
    hash_loop(&mut hasher, &outline);

    hash_u64(&mut hasher, cutouts.len() as u64);
    for cutout in &cutouts {
        hash_loop(&mut hasher, cutout);
    }

    hash_u64(&mut hasher, holes.len() as u64);
    for hole in &holes {
        hash_hole(&mut hasher, hole);
    }

    hash_u64(&mut hasher, keepouts.len() as u64);
    for keepout in &keepouts {
        hash_keepout(&mut hasher, keepout);
    }

    let digest = hasher.finalize();
    let mut out = String::with_capacity(HASH_PREFIX.len() + 64);
    out.push_str(HASH_PREFIX);
    for b in digest {
        use std::fmt::Write;
        let _ = write!(&mut out, "{b:02x}");
    }
    out
}

/// Normalize a closed loop's traversal direction (positive signed area in Y-down coordinates) and
/// cyclic starting element (lexicographically minimal rotation).
pub fn canonicalize_loop(lp: &Loop) -> Loop {
    if lp.elements.is_empty() {
        return Loop::new(Vec::new());
    }
    if lp.elements.len() == 1 && matches!(lp.elements[0].kind, ElementKind::Circle { .. }) {
        return lp.clone();
    }

    let walked = ensure_walked(lp);
    let area = signed_area_nm2(&walked.elements);
    if area > 1.0 {
        min_rotation(&walked.elements)
    } else if area < -1.0 {
        min_rotation(&walked.reversed().elements)
    } else {
        let fwd = min_rotation(&walked.elements);
        let rev = min_rotation(&walked.reversed().elements);
        if fwd <= rev { fwd } else { rev }
    }
}

/// If `lp`'s elements are not already ordered head-to-tail, attempt to chain them deterministically
/// by exact endpoint equality so an unchained in-memory `Loop` still canonicalizes cleanly.
fn ensure_walked(lp: &Loop) -> Loop {
    let elems = &lp.elements;
    let n = elems.len();
    if n <= 1 {
        return lp.clone();
    }
    let already_chained = (0..n).all(|i| {
        match (
            elems[i].kind.endpoints(),
            elems[(i + 1) % n].kind.endpoints(),
        ) {
            (Some((_, end)), Some((next_start, _))) => end == next_start,
            _ => false,
        }
    });
    if already_chained {
        return lp.clone();
    }

    // Attempt exact endpoint walk starting from index 0.
    let mut used = vec![false; n];
    let mut out = Vec::with_capacity(n);
    let Some((first_a, first_b)) = elems[0].kind.endpoints() else {
        return lp.clone();
    };
    used[0] = true;
    out.push(elems[0].clone());
    let mut cur_end = first_b;
    for _ in 1..n {
        let found = (0..n).find_map(|idx| {
            if used[idx] {
                return None;
            }
            let (a, b) = elems[idx].kind.endpoints()?;
            if a == cur_end {
                Some((idx, elems[idx].clone(), b))
            } else if b == cur_end {
                Some((idx, elems[idx].reversed(), a))
            } else {
                None
            }
        });
        match found {
            Some((idx, el, next_end)) => {
                used[idx] = true;
                out.push(el);
                cur_end = next_end;
            }
            None => return lp.clone(),
        }
    }
    if cur_end == first_a {
        Loop::new(out)
    } else {
        lp.clone()
    }
}

fn min_rotation(elems: &[Element]) -> Loop {
    let n = elems.len();
    if n == 0 {
        return Loop::new(Vec::new());
    }
    let mut best_idx = 0usize;
    for candidate in 1..n {
        if rotation_cmp(elems, candidate, best_idx).is_lt() {
            best_idx = candidate;
        }
    }
    let mut rotated = Vec::with_capacity(n);
    rotated.extend_from_slice(&elems[best_idx..]);
    rotated.extend_from_slice(&elems[..best_idx]);
    Loop::new(rotated)
}

fn rotation_cmp(elems: &[Element], a: usize, b: usize) -> std::cmp::Ordering {
    let n = elems.len();
    for k in 0..n {
        let ord = elems[(a + k) % n].cmp(&elems[(b + k) % n]);
        if !ord.is_eq() {
            return ord;
        }
    }
    std::cmp::Ordering::Equal
}

/// Angle of `p` around `center` in KiCad's Y-down frame (`atan2(dy, dx)`).
pub(crate) fn angle_ydown(center: Point, p: Point) -> f64 {
    ((p.y - center.y) as f64).atan2((p.x - center.x) as f64)
}

/// Positive angular sweep in `(0, 2π]` of an arc in KiCad's Y-down frame.
pub(crate) fn sweep_ydown(center: Point, start: Point, end: Point, angle_increasing: bool) -> f64 {
    let tau = std::f64::consts::TAU;
    let d = (angle_ydown(center, end) - angle_ydown(center, start)).rem_euclid(tau);
    let d = if angle_increasing {
        d
    } else {
        (tau - d).rem_euclid(tau)
    };
    if d == 0.0 { tau } else { d }
}

/// Signed area in `nm²` of a walked loop in KiCad's Y-down frame (positive = angle-increasing).
pub(crate) fn signed_area_nm2(elems: &[Element]) -> f64 {
    let mut shoelace = 0.0_f64;
    let mut bulge = 0.0_f64;
    for el in elems {
        match el.kind {
            ElementKind::Line { start, end } => {
                shoelace += (start.x as f64) * (end.y as f64) - (end.x as f64) * (start.y as f64);
            }
            ElementKind::Arc {
                center,
                start,
                end,
                angle_increasing,
            } => {
                shoelace += (start.x as f64) * (end.y as f64) - (end.x as f64) * (start.y as f64);
                let r = ((start.x - center.x) as f64).hypot((start.y - center.y) as f64);
                let th = sweep_ydown(center, start, end, angle_increasing);
                let seg = 0.5 * r * r * (th - th.sin());
                bulge += if angle_increasing { seg } else { -seg };
            }
            ElementKind::Circle { diameter_nm, .. } => {
                let r = (diameter_nm as f64) * 0.5;
                return std::f64::consts::PI * r * r;
            }
        }
    }
    0.5 * shoelace + bulge
}

fn hash_u8(h: &mut Sha256, v: u8) {
    h.update([v]);
}

fn hash_bool(h: &mut Sha256, v: bool) {
    hash_u8(h, u8::from(v));
}

fn hash_u64(h: &mut Sha256, v: u64) {
    h.update(v.to_le_bytes());
}

fn hash_i64(h: &mut Sha256, v: i64) {
    h.update(v.to_le_bytes());
}

fn hash_opt_str(h: &mut Sha256, s: Option<&str>) {
    match s {
        None => hash_u8(h, 0),
        Some(text) => {
            hash_u8(h, 1);
            hash_u64(h, text.len() as u64);
            h.update(text.as_bytes());
        }
    }
}

fn hash_point(h: &mut Sha256, p: Point) {
    hash_i64(h, p.x);
    hash_i64(h, p.y);
}

fn hash_element(h: &mut Sha256, el: &Element) {
    hash_opt_str(h, el.id.as_deref());
    match el.kind {
        ElementKind::Line { start, end } => {
            hash_u8(h, 1);
            hash_point(h, start);
            hash_point(h, end);
        }
        ElementKind::Arc {
            center,
            start,
            end,
            angle_increasing,
        } => {
            hash_u8(h, 2);
            hash_point(h, center);
            hash_point(h, start);
            hash_point(h, end);
            hash_bool(h, angle_increasing);
        }
        ElementKind::Circle {
            center,
            diameter_nm,
        } => {
            hash_u8(h, 3);
            hash_point(h, center);
            hash_i64(h, diameter_nm);
        }
    }
}

fn hash_loop(h: &mut Sha256, lp: &Loop) {
    hash_u64(h, lp.elements.len() as u64);
    for el in &lp.elements {
        hash_element(h, el);
    }
}

fn hash_hole(h: &mut Sha256, hole: &Hole) {
    hash_opt_str(h, hole.id.as_deref());
    hash_point(h, hole.center);
    hash_i64(h, hole.diameter_nm);
    hash_bool(h, hole.plated);
    hash_i64(h, hole.clearance_nm);
}

fn hash_side(h: &mut Sha256, side: Side) {
    let tag = match side {
        Side::Both => 0,
        Side::Front => 1,
        Side::Back => 2,
    };
    hash_u8(h, tag);
}

fn hash_keepout(h: &mut Sha256, keepout: &Keepout) {
    hash_loop(h, &keepout.boundary);
    hash_side(h, keepout.side);
    match keepout.max_height_nm {
        None => hash_u8(h, 0),
        Some(nm) => {
            hash_u8(h, 1);
            hash_i64(h, nm);
        }
    }
}
