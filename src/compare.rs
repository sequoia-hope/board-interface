use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use crate::hash::{canonicalize_loop, hash};
use crate::model::{Element, ElementKind, Hole, Keepout, Loop, Manifest, Point, Record, Side};

/// Maximum translation distance (`5 mm`) at which a single isolated, un-ID'd item of identical
/// size and orientation is matched by geometry rather than reported as uncertain.
const UNAMBIGUOUS_MOVE_LIMIT_NM: i64 = 5_000_000;

/// Sub-micrometre tolerance (`100 nm`) for exact positional re-matching after a third-party CAD
/// re-save without extended data.
const RESAVE_MATCH_TOL_NM: i64 = 100;

/// Sync and handover classification between `base` and `theirs`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Classification {
    /// Same revision, same driver, matching manifest and geometry hash.
    InSync,
    /// `theirs` was published by the same driver at a newer revision.
    UpdatedByDriver,
    /// `base` was published directly on top of `theirs` (`base.parent == Some(theirs.revision)`), or
    /// `base`'s driver took the drive from `theirs` (`base.last_handover.from_revision ==
    /// theirs.revision`) and may have published since.
    TheirsBehind,
    /// Valid drive handover: `theirs.driver != base.driver` and `theirs.parent` equals
    /// `base.revision` (the previous driver's latest revision).
    CleanHandover,
    /// Invalid handover from an older revision (`theirs.driver != base.driver` and `theirs.parent`
    /// is not `base.revision`, while one side simply continued driving).
    StaleTakeOver,
    /// Two revisions branched from the same `parent` before seeing each other (both took over from
    /// the same revision, or two divergent publishes share a parent).
    CompetingTakeOvers,
    /// `theirs` does not match its own manifest (`hash(theirs) != theirs.manifest.geometry_hash`
    /// or extended data IDs were stripped by an external CAD re-save).
    ChangedOutsideSuite,
}

/// Functional role of a board-shape item.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Role {
    Outline,
    Cutout,
    Hole,
    Keepout,
}

/// Per-element change classification.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ChangeKind {
    Added,
    Removed,
    Moved { dx_nm: i64, dy_nm: i64 },
    Changed,
}

/// One element-level or hole-level difference between `base` and `theirs`.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub struct Change {
    pub id: Option<String>,
    pub role: Role,
    pub kind: ChangeKind,
}

/// An ID-less item in `theirs` that was unambiguously re-matched by geometry to an item in `base`.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub struct RematchedId {
    pub role: Role,
    /// Ordinal of the item within its role in `theirs`.
    pub theirs_index: usize,
    /// Recovered stable ID from `base`.
    pub base_id: String,
}

/// Ambiguous correspondence when extended data is missing and geometry alone cannot safely decide
/// which `base` IDs map to which `theirs` items without risking broken mates.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub struct UncertainMatch {
    pub role: Role,
    pub candidate_base_ids: Vec<String>,
    pub theirs_indices: Vec<usize>,
    pub reason: String,
}

/// Result of [`compare`]`(base, theirs)`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Comparison {
    pub classification: Classification,
    pub changes: Vec<Change>,
    pub thickness_changed: Option<(i64, i64)>,
    pub rematched: Vec<RematchedId>,
    pub uncertain: Vec<UncertainMatch>,
}

/// Compare `theirs` against `base`, returning the sync/handover [`Classification`], deterministic
/// per-ID [`Change`]s, and any geometric re-matches or [`UncertainMatch`]es when extended data was
/// lost on re-save.
pub fn compare(base: &Record, theirs: &Record) -> Comparison {
    let classification = classify(base, theirs);

    let base_t = base.thickness_nm();
    let theirs_t = theirs.thickness_nm();
    let thickness_changed = (base_t != theirs_t).then_some((base_t, theirs_t));

    let base_items = collect_items(base);
    let theirs_items = collect_items(theirs);

    let mut base_by_id: BTreeMap<String, usize> = BTreeMap::new();
    for (idx, item) in base_items.iter().enumerate() {
        if let Some(id) = &item.id {
            base_by_id.insert(id.clone(), idx);
        }
    }

    let mut theirs_by_id: BTreeMap<String, usize> = BTreeMap::new();
    for (idx, item) in theirs_items.iter().enumerate() {
        if let Some(id) = &item.id {
            theirs_by_id.insert(id.clone(), idx);
        }
    }

    let mut base_matched = vec![false; base_items.len()];
    let mut theirs_matched = vec![false; theirs_items.len()];
    let mut changes: Vec<Change> = Vec::new();
    let mut rematched: Vec<RematchedId> = Vec::new();
    let mut uncertain: Vec<UncertainMatch> = Vec::new();

    // 1. Direct ID matches (in deterministic BTreeMap key order).
    for (id, &b_idx) in &base_by_id {
        if let Some(&t_idx) = theirs_by_id.get(id) {
            base_matched[b_idx] = true;
            theirs_matched[t_idx] = true;
            let b_item = &base_items[b_idx];
            let t_item = &theirs_items[t_idx];
            if let Some(kind) = diff_matched_item(b_item, t_item, true) {
                changes.push(Change {
                    id: Some(id.clone()),
                    role: t_item.role,
                    kind,
                });
            }
        }
    }

    // Any item in `theirs` that has an explicit `Some(id)` NOT in `base` is unambiguously Added.
    for (id, &t_idx) in &theirs_by_id {
        if !theirs_matched[t_idx] {
            theirs_matched[t_idx] = true;
            changes.push(Change {
                id: Some(id.clone()),
                role: theirs_items[t_idx].role,
                kind: ChangeKind::Added,
            });
        }
    }

    // 2. Geometric re-matching for ID-less items in `theirs` (e.g. re-saved by another CAD).
    let has_idless_theirs = theirs_items
        .iter()
        .enumerate()
        .any(|(i, it)| !theirs_matched[i] && it.id.is_none());

    if !has_idless_theirs {
        // All `theirs` items carried IDs, so any remaining unmatched `base` items were Removed.
        for (b_idx, b_item) in base_items.iter().enumerate() {
            if !base_matched[b_idx] {
                changes.push(Change {
                    id: b_item.id.clone(),
                    role: b_item.role,
                    kind: ChangeKind::Removed,
                });
            }
        }
    } else {
        for role in [Role::Outline, Role::Cutout, Role::Hole, Role::Keepout] {
            match_idless_for_role(
                role,
                &base_items,
                &theirs_items,
                &mut base_matched,
                &mut theirs_matched,
                &mut changes,
                &mut rematched,
                &mut uncertain,
            );
        }
    }

    changes.sort();
    rematched.sort();
    uncertain.sort();

    Comparison {
        classification,
        changes,
        thickness_changed,
        rematched,
        uncertain,
    }
}

/// Check whether `a` and `b` are identical within `tolerance_nm` on every coordinate, diameter,
/// clearance, keepout height, and board thickness (the "publish nothing when nothing moved" rule).
pub fn within(a: &Record, b: &Record, tolerance_nm: i64) -> bool {
    if tolerance_nm < 0 {
        return false;
    }
    if (a.thickness_nm() - b.thickness_nm()).abs() > tolerance_nm {
        return false;
    }
    if a.manifest.approximate != b.manifest.approximate {
        return false;
    }
    if a.outline.elements.len() != b.outline.elements.len()
        || a.cutouts.len() != b.cutouts.len()
        || a.holes.len() != b.holes.len()
        || a.keepouts.len() != b.keepouts.len()
    {
        return false;
    }

    let all_have_ids = all_items_have_ids(a) && all_items_have_ids(b);
    if all_have_ids {
        within_by_id(a, b, tolerance_nm)
    } else {
        within_canonical(a, b, tolerance_nm)
    }
}

fn classify(base: &Record, theirs: &Record) -> Classification {
    let theirs_actual_hash = hash(theirs);
    if theirs_actual_hash != theirs.manifest.geometry_hash || !all_items_have_ids(theirs) {
        return Classification::ChangedOutsideSuite;
    }

    let base_hash = hash(base);
    let bm = &base.manifest;
    let tm = &theirs.manifest;

    if tm.revision == bm.revision && tm.driver == bm.driver && theirs_actual_hash == base_hash {
        return Classification::InSync;
    }

    // Check if `base` was published directly on top of `theirs`, or took the drive from `theirs`
    // and has published since (the mirror of `took_over_from_base` below).
    let base_took_over_from_theirs = bm
        .last_handover
        .as_ref()
        .is_some_and(|h| h.by == bm.driver && h.from_revision == tm.revision);
    if bm.parent.as_deref() == Some(tm.revision.as_str()) || base_took_over_from_theirs {
        return Classification::TheirsBehind;
    }

    // Clean handover: the drive was taken from `base`'s latest revision, either in `theirs` itself
    // or earlier, with the new driver having published since.
    let took_over_from_base = tm
        .last_handover
        .as_ref()
        .is_some_and(|h| h.by == tm.driver && h.from_revision == bm.revision);
    if tm.driver != bm.driver
        && (tm.parent.as_deref() == Some(bm.revision.as_str()) || took_over_from_base)
    {
        return Classification::CleanHandover;
    }

    // Two distinct revisions with the exact same parent:
    if bm.parent.is_some() && bm.parent == tm.parent && bm.revision != tm.revision {
        if tm.driver == bm.driver || (is_handover_step(bm) && is_handover_step(tm)) {
            return Classification::CompetingTakeOvers;
        }
        return Classification::StaleTakeOver;
    }

    if tm.driver != bm.driver {
        return Classification::StaleTakeOver;
    }

    Classification::UpdatedByDriver
}

// `author` is provenance and is not hashed into `revision`, so classification must not read it:
// a take-over publish is the one whose `parent` is the revision it took the drive from.
fn is_handover_step(m: &Manifest) -> bool {
    m.parent.is_some()
        && m.last_handover.as_ref().is_some_and(|h| {
            h.by == m.driver && m.parent.as_deref() == Some(h.from_revision.as_str())
        })
}

fn all_items_have_ids(record: &Record) -> bool {
    record.outline.elements.iter().all(|e| e.id.is_some())
        && record
            .cutouts
            .iter()
            .all(|lp| lp.elements.iter().all(|e| e.id.is_some()))
        && record.holes.iter().all(|h| h.id.is_some())
        && record
            .keepouts
            .iter()
            .all(|k| k.boundary.elements.iter().all(|e| e.id.is_some()))
}

#[derive(Debug, Clone)]
struct FlatItem {
    id: Option<String>,
    role: Role,
    role_index: usize,
    geom: ElementKind,
    attrs: ItemAttrs,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ItemAttrs {
    None,
    Hole {
        plated: bool,
        clearance_nm: i64,
    },
    Keepout {
        side: Side,
        max_height_nm: Option<i64>,
    },
}

fn collect_items(record: &Record) -> Vec<FlatItem> {
    let mut out = Vec::new();

    for (idx, el) in record.outline.elements.iter().enumerate() {
        out.push(FlatItem {
            id: el.id.clone(),
            role: Role::Outline,
            role_index: idx,
            geom: el.kind.undirected_canonical(),
            attrs: ItemAttrs::None,
        });
    }

    let mut cutout_idx = 0usize;
    for lp in &record.cutouts {
        for el in &lp.elements {
            out.push(FlatItem {
                id: el.id.clone(),
                role: Role::Cutout,
                role_index: cutout_idx,
                geom: el.kind.undirected_canonical(),
                attrs: ItemAttrs::None,
            });
            cutout_idx += 1;
        }
    }

    for (idx, h) in record.holes.iter().enumerate() {
        out.push(FlatItem {
            id: h.id.clone(),
            role: Role::Hole,
            role_index: idx,
            geom: ElementKind::Circle {
                center: h.center,
                diameter_nm: h.diameter_nm,
            },
            attrs: ItemAttrs::Hole {
                plated: h.plated,
                clearance_nm: h.clearance_nm,
            },
        });
    }

    let mut keepout_idx = 0usize;
    for k in &record.keepouts {
        for el in &k.boundary.elements {
            out.push(FlatItem {
                id: el.id.clone(),
                role: Role::Keepout,
                role_index: keepout_idx,
                geom: el.kind.undirected_canonical(),
                attrs: ItemAttrs::Keepout {
                    side: k.side,
                    max_height_nm: k.max_height_nm,
                },
            });
            keepout_idx += 1;
        }
    }

    out
}

fn diff_matched_item(
    base: &FlatItem,
    theirs: &FlatItem,
    compare_attrs: bool,
) -> Option<ChangeKind> {
    if base.role != theirs.role {
        return Some(ChangeKind::Changed);
    }
    if compare_attrs && base.attrs != theirs.attrs {
        return Some(ChangeKind::Changed);
    }
    if base.geom == theirs.geom {
        return None;
    }
    if let Some((dx, dy)) = pure_translation(base.geom, theirs.geom) {
        return Some(ChangeKind::Moved {
            dx_nm: dx,
            dy_nm: dy,
        });
    }
    Some(ChangeKind::Changed)
}

/// Return `Some((dx, dy))` if `b` is a non-zero rigid translation of `a` with identical shape,
/// size, and orientation (considering both traversal senses for open segments).
fn pure_translation(a: ElementKind, b: ElementKind) -> Option<(i64, i64)> {
    match (a, b) {
        (
            ElementKind::Circle {
                center: ca,
                diameter_nm: da,
            },
            ElementKind::Circle {
                center: cb,
                diameter_nm: db,
            },
        ) => {
            if da != db {
                return None;
            }
            let dx = cb.x - ca.x;
            let dy = cb.y - ca.y;
            (dx != 0 || dy != 0).then_some((dx, dy))
        }
        (ElementKind::Line { start: sa, end: ea }, ElementKind::Line { start: sb, end: eb }) => {
            for (s2, e2) in [(sb, eb), (eb, sb)] {
                let dx1 = s2.x - sa.x;
                let dy1 = s2.y - sa.y;
                let dx2 = e2.x - ea.x;
                let dy2 = e2.y - ea.y;
                if dx1 == dx2 && dy1 == dy2 && (dx1 != 0 || dy1 != 0) {
                    return Some((dx1, dy1));
                }
            }
            None
        }
        (
            ElementKind::Arc {
                center: ca,
                start: sa,
                end: ea,
                angle_increasing: ccw_a,
            },
            ElementKind::Arc {
                center: cb,
                start: sb,
                end: eb,
                angle_increasing: ccw_b,
            },
        ) => {
            for (s2, e2, ccw2) in [(sb, eb, ccw_b), (eb, sb, !ccw_b)] {
                if ccw_a != ccw2 {
                    continue;
                }
                let dxc = cb.x - ca.x;
                let dyc = cb.y - ca.y;
                if s2.x - sa.x == dxc
                    && s2.y - sa.y == dyc
                    && e2.x - ea.x == dxc
                    && e2.y - ea.y == dyc
                    && (dxc != 0 || dyc != 0)
                {
                    return Some((dxc, dyc));
                }
            }
            None
        }
        _ => None,
    }
}

#[allow(clippy::too_many_arguments)]
fn match_idless_for_role(
    role: Role,
    base_items: &[FlatItem],
    theirs_items: &[FlatItem],
    base_matched: &mut [bool],
    theirs_matched: &mut [bool],
    changes: &mut Vec<Change>,
    rematched: &mut Vec<RematchedId>,
    uncertain: &mut Vec<UncertainMatch>,
) {
    let rem_base: Vec<usize> = base_items
        .iter()
        .enumerate()
        .filter(|(i, it)| !base_matched[*i] && it.role == role)
        .map(|(i, _)| i)
        .collect();
    let rem_theirs: Vec<usize> = theirs_items
        .iter()
        .enumerate()
        .filter(|(i, it)| !theirs_matched[*i] && it.role == role)
        .map(|(i, _)| i)
        .collect();

    // Pass 1: unique positional match within `RESAVE_MATCH_TOL_NM` (100 nm).
    for &t_idx in &rem_theirs {
        if theirs_matched[t_idx] {
            continue;
        }
        let t_geom = theirs_items[t_idx].geom;
        let candidates: Vec<usize> = rem_base
            .iter()
            .copied()
            .filter(|&b_idx| {
                !base_matched[b_idx]
                    && geom_within(base_items[b_idx].geom, t_geom, RESAVE_MATCH_TOL_NM)
            })
            .collect();
        if candidates.len() == 1 {
            let b_idx = candidates[0];
            // Also verify no other unmatched `theirs` item sits within 100 nm of `b_idx`.
            let reverse_candidates = rem_theirs
                .iter()
                .filter(|&&other_t| {
                    !theirs_matched[other_t]
                        && geom_within(
                            base_items[b_idx].geom,
                            theirs_items[other_t].geom,
                            RESAVE_MATCH_TOL_NM,
                        )
                })
                .count();
            if reverse_candidates == 1 {
                base_matched[b_idx] = true;
                theirs_matched[t_idx] = true;
                if let Some(base_id) = &base_items[b_idx].id {
                    rematched.push(RematchedId {
                        role,
                        theirs_index: theirs_items[t_idx].role_index,
                        base_id: base_id.clone(),
                    });
                }
                if let Some(kind) =
                    diff_matched_item(&base_items[b_idx], &theirs_items[t_idx], false)
                {
                    changes.push(Change {
                        id: base_items[b_idx].id.clone(),
                        role,
                        kind,
                    });
                }
            }
        }
    }

    let still_base: Vec<usize> = rem_base.into_iter().filter(|&i| !base_matched[i]).collect();
    let still_theirs: Vec<usize> = rem_theirs
        .into_iter()
        .filter(|&i| !theirs_matched[i])
        .collect();

    if still_base.is_empty() && still_theirs.is_empty() {
        return;
    }

    // If only `base` items remain (none in `theirs`), they were unambiguously Removed.
    if still_theirs.is_empty() {
        for b_idx in still_base {
            base_matched[b_idx] = true;
            changes.push(Change {
                id: base_items[b_idx].id.clone(),
                role,
                kind: ChangeKind::Removed,
            });
        }
        return;
    }

    // If only `theirs` items remain (none in `base`), they were unambiguously Added.
    if still_base.is_empty() {
        for t_idx in still_theirs {
            theirs_matched[t_idx] = true;
            changes.push(Change {
                id: None,
                role,
                kind: ChangeKind::Added,
            });
        }
        return;
    }

    // Pass 2: if there is SOLELY ONE unmatched item in `base` and SOLELY ONE unmatched item in
    // `theirs` for this role, AND it is a small pure translation (`<= 5 mm`) of the exact same
    // curve size/orientation, match it as `Moved`. Otherwise report as uncertain rather than
    // guessing!
    if still_base.len() == 1 && still_theirs.len() == 1 {
        let b_idx = still_base[0];
        let t_idx = still_theirs[0];
        if let Some((dx, dy)) = pure_translation_within_shape_tol(
            base_items[b_idx].geom,
            theirs_items[t_idx].geom,
            RESAVE_MATCH_TOL_NM,
        ) && dx.abs() <= UNAMBIGUOUS_MOVE_LIMIT_NM
            && dy.abs() <= UNAMBIGUOUS_MOVE_LIMIT_NM
        {
            base_matched[b_idx] = true;
            theirs_matched[t_idx] = true;
            if let Some(base_id) = &base_items[b_idx].id {
                rematched.push(RematchedId {
                    role,
                    theirs_index: theirs_items[t_idx].role_index,
                    base_id: base_id.clone(),
                });
            }
            changes.push(Change {
                id: base_items[b_idx].id.clone(),
                role,
                kind: ChangeKind::Moved {
                    dx_nm: dx,
                    dy_nm: dy,
                },
            });
            return;
        }
    }

    // Otherwise: multiple unmatched items or a shape/large-jump change without IDs — report
    // uncertain rather than guessing.
    let mut candidate_base_ids: Vec<String> = still_base
        .iter()
        .filter_map(|&b_idx| {
            base_matched[b_idx] = true;
            base_items[b_idx].id.clone()
        })
        .collect();
    candidate_base_ids.sort();

    let mut theirs_indices: Vec<usize> = still_theirs
        .iter()
        .map(|&t_idx| {
            theirs_matched[t_idx] = true;
            theirs_items[t_idx].role_index
        })
        .collect();
    theirs_indices.sort_unstable();

    uncertain.push(UncertainMatch {
        role,
        candidate_base_ids,
        theirs_indices,
        reason: "extended data IDs are missing and geometry changes have multiple or non-trivial interpretations".to_string(),
    });
}

fn pure_translation_within_shape_tol(
    a: ElementKind,
    b: ElementKind,
    shape_tol_nm: i64,
) -> Option<(i64, i64)> {
    if let Some(exact) = pure_translation(a, b) {
        return Some(exact);
    }
    if let (
        ElementKind::Circle {
            center: ca,
            diameter_nm: da,
        },
        ElementKind::Circle {
            center: cb,
            diameter_nm: db,
        },
    ) = (a, b)
        && (da - db).abs() <= shape_tol_nm
    {
        let dx = cb.x - ca.x;
        let dy = cb.y - ca.y;
        if dx != 0 || dy != 0 {
            return Some((dx, dy));
        }
    }
    None
}

fn within_by_id(a: &Record, b: &Record, tol: i64) -> bool {
    let items_a = collect_items(a);
    let items_b = collect_items(b);
    if items_a.len() != items_b.len() {
        return false;
    }

    let mut map_b: BTreeMap<&str, &FlatItem> = BTreeMap::new();
    for item in &items_b {
        let Some(id) = item.id.as_deref() else {
            return false;
        };
        if map_b.insert(id, item).is_some() {
            return false;
        }
    }

    for ia in &items_a {
        let Some(id) = ia.id.as_deref() else {
            return false;
        };
        let Some(&ib) = map_b.get(id) else {
            return false;
        };
        if ia.role != ib.role {
            return false;
        }
        if !attrs_within(ia.attrs, ib.attrs, tol) {
            return false;
        }
        if !geom_within(ia.geom, ib.geom, tol) {
            return false;
        }
    }

    // Verify loop grouping for cutouts and keepouts matches too.
    loop_id_sets(&a.cutouts) == loop_id_sets(&b.cutouts)
        && keepout_id_sets(&a.keepouts) == keepout_id_sets(&b.keepouts)
}

fn loop_id_sets(loops: &[Loop]) -> Vec<Vec<String>> {
    let mut sets: Vec<Vec<String>> = loops
        .iter()
        .map(|lp| {
            let mut ids: Vec<String> = lp.elements.iter().filter_map(|e| e.id.clone()).collect();
            ids.sort();
            ids
        })
        .collect();
    sets.sort();
    sets
}

fn keepout_id_sets(keepouts: &[Keepout]) -> Vec<(Side, Vec<String>)> {
    let mut sets: Vec<(Side, Vec<String>)> = keepouts
        .iter()
        .map(|k| {
            let mut ids: Vec<String> = k
                .boundary
                .elements
                .iter()
                .filter_map(|e| e.id.clone())
                .collect();
            ids.sort();
            (k.side, ids)
        })
        .collect();
    sets.sort();
    sets
}

fn within_canonical(a: &Record, b: &Record, tol: i64) -> bool {
    let out_a = canonicalize_loop(&a.outline);
    let out_b = canonicalize_loop(&b.outline);
    if !loop_within(&out_a, &out_b, tol) {
        return false;
    }

    let mut cut_a: Vec<Loop> = a.cutouts.iter().map(canonicalize_loop).collect();
    let mut cut_b: Vec<Loop> = b.cutouts.iter().map(canonicalize_loop).collect();
    cut_a.sort();
    cut_b.sort();
    if !cut_a
        .iter()
        .zip(&cut_b)
        .all(|(la, lb)| loop_within(la, lb, tol))
    {
        return false;
    }

    let mut holes_a: Vec<Hole> = a.holes.clone();
    let mut holes_b: Vec<Hole> = b.holes.clone();
    holes_a.sort();
    holes_b.sort();
    if !holes_a
        .iter()
        .zip(&holes_b)
        .all(|(ha, hb)| hole_within(ha, hb, tol))
    {
        return false;
    }

    let mut ko_a: Vec<Keepout> = a
        .keepouts
        .iter()
        .map(|k| Keepout {
            boundary: canonicalize_loop(&k.boundary),
            side: k.side,
            max_height_nm: k.max_height_nm,
        })
        .collect();
    let mut ko_b: Vec<Keepout> = b
        .keepouts
        .iter()
        .map(|k| Keepout {
            boundary: canonicalize_loop(&k.boundary),
            side: k.side,
            max_height_nm: k.max_height_nm,
        })
        .collect();
    ko_a.sort();
    ko_b.sort();
    ko_a.iter().zip(&ko_b).all(|(ka, kb)| {
        ka.side == kb.side
            && opt_nm_within(ka.max_height_nm, kb.max_height_nm, tol)
            && loop_within(&ka.boundary, &kb.boundary, tol)
    })
}

fn loop_within(a: &Loop, b: &Loop, tol: i64) -> bool {
    if a.elements.len() != b.elements.len() {
        return false;
    }
    let n = a.elements.len();
    if n == 0 {
        return true;
    }
    // Try cyclic shifts of `b` (and `b.reversed()`) in case a 1 nm boundary flip changed the
    // tie-broken minimal rotation vertex!
    for candidate in [b.clone(), b.reversed()] {
        for shift in 0..n {
            let ok = (0..n).all(|k| {
                let ea = &a.elements[k];
                let eb = &candidate.elements[(k + shift) % n];
                ea.id == eb.id && directed_geom_within(ea.kind, eb.kind, tol)
            });
            if ok {
                return true;
            }
        }
    }
    false
}

fn hole_within(a: &Hole, b: &Hole, tol: i64) -> bool {
    a.id == b.id
        && a.plated == b.plated
        && (a.clearance_nm - b.clearance_nm).abs() <= tol
        && (a.diameter_nm - b.diameter_nm).abs() <= tol
        && point_within(a.center, b.center, tol)
}

fn attrs_within(a: ItemAttrs, b: ItemAttrs, tol: i64) -> bool {
    match (a, b) {
        (ItemAttrs::None, ItemAttrs::None) => true,
        (
            ItemAttrs::Hole {
                plated: pa,
                clearance_nm: ca,
            },
            ItemAttrs::Hole {
                plated: pb,
                clearance_nm: cb,
            },
        ) => pa == pb && (ca - cb).abs() <= tol,
        (
            ItemAttrs::Keepout {
                side: sa,
                max_height_nm: ha,
            },
            ItemAttrs::Keepout {
                side: sb,
                max_height_nm: hb,
            },
        ) => sa == sb && opt_nm_within(ha, hb, tol),
        _ => false,
    }
}

fn opt_nm_within(a: Option<i64>, b: Option<i64>, tol: i64) -> bool {
    match (a, b) {
        (None, None) => true,
        (Some(va), Some(vb)) => (va - vb).abs() <= tol,
        _ => false,
    }
}

fn geom_within(a: ElementKind, b: ElementKind, tol: i64) -> bool {
    directed_geom_within(a, b, tol) || directed_geom_within(a, b.reversed(), tol)
}

fn directed_geom_within(a: ElementKind, b: ElementKind, tol: i64) -> bool {
    match (a, b) {
        (ElementKind::Line { start: sa, end: ea }, ElementKind::Line { start: sb, end: eb }) => {
            point_within(sa, sb, tol) && point_within(ea, eb, tol)
        }
        (
            ElementKind::Arc {
                center: ca,
                start: sa,
                end: ea,
                angle_increasing: ccw_a,
            },
            ElementKind::Arc {
                center: cb,
                start: sb,
                end: eb,
                angle_increasing: ccw_b,
            },
        ) => {
            ccw_a == ccw_b
                && point_within(ca, cb, tol)
                && point_within(sa, sb, tol)
                && point_within(ea, eb, tol)
        }
        (
            ElementKind::Circle {
                center: ca,
                diameter_nm: da,
            },
            ElementKind::Circle {
                center: cb,
                diameter_nm: db,
            },
        ) => point_within(ca, cb, tol) && (da - db).abs() <= tol,
        _ => false,
    }
}

fn point_within(a: Point, b: Point, tol: i64) -> bool {
    (a.x - b.x).abs() <= tol && (a.y - b.y).abs() <= tol
}

#[allow(dead_code)]
fn _elem_ref(e: &Element) -> &Element {
    e
}
