#!/usr/bin/env python3
"""Convert the Edge.Cuts outline of easy2350.kicad_pcb into board-interface golden files.

Reads a KiCad .kicad_pcb S-expression (read-only), extracts board thickness and all Edge.Cuts
primitives (gr_line, gr_arc, gr_circle, gr_rect), chains them into the outer board loop in KiCad's
Y-down nanometre frame, computes the canonical board-interface v1 SHA-256 geometry hash, and writes
golden/easy2350.dxf and golden/easy2350.json.
"""

from __future__ import annotations

import argparse
import hashlib
import json
import math
import struct
from pathlib import Path

DEFAULT_PCB = Path(
    "/home/sequoia/Software/boardroom/examples/easy2350/hw/easy2350/a/easy2350.kicad_pcb"
)
DEFAULT_OUT_DIR = Path(__file__).resolve().parent.parent / "golden"
SOURCE_COMMIT = "56d2bd8fcb94ff5f1137e66a203efa30dc7c91e4"
PUBLISHED_AT = "2026-11-02T23:58:12Z"


def tokenize_sexpr(text: str) -> list[str]:
    tokens: list[str] = []
    i = 0
    n = len(text)
    while i < n:
        c = text[i]
        if c.isspace():
            i += 1
        elif c in ("(", ")"):
            tokens.append(c)
            i += 1
        elif c == '"':
            j = i + 1
            buf: list[str] = []
            while j < n and text[j] != '"':
                if text[j] == "\\" and j + 1 < n:
                    j += 1
                buf.append(text[j])
                j += 1
            tokens.append("".join(buf))
            i = j + 1
        else:
            j = i
            while j < n and not text[j].isspace() and text[j] not in ("(", ")", '"'):
                j += 1
            tokens.append(text[i:j])
            i = j
    return tokens


def parse_sexpr(text: str):
    tokens = tokenize_sexpr(text)
    stack: list[list] = [[]]
    for tok in tokens:
        if tok == "(":
            node: list = []
            stack[-1].append(node)
            stack.append(node)
        elif tok == ")":
            stack.pop()
        else:
            stack[-1].append(tok)
    return stack[0][0]


def child_form(node: list, head: str) -> list | None:
    for item in node[1:]:
        if isinstance(item, list) and item and item[0] == head:
            return item
    return None


def mm_to_nm(val_str: str) -> int:
    return round(float(val_str) * 1_000_000)


def circumcenter_mm(a: tuple[float, float], b: tuple[float, float], c: tuple[float, float]):
    bx, by = b[0] - a[0], b[1] - a[1]
    cx, cy = c[0] - a[0], c[1] - a[1]
    d = 2.0 * (bx * cy - by * cx)
    b2 = bx * bx + by * by
    c2 = cx * cx + cy * cy
    ux = (cy * b2 - by * c2) / d
    uy = (bx * c2 - cx * b2) / d
    return (a[0] + ux, a[1] + uy)


def arc_is_ccw_ydown(
    center: tuple[float, float],
    start: tuple[float, float],
    mid: tuple[float, float],
    end: tuple[float, float],
) -> bool:
    tau = 2.0 * math.pi
    a0 = math.atan2(start[1] - center[1], start[0] - center[0])
    dm = (math.atan2(mid[1] - center[1], mid[0] - center[0]) - a0) % tau
    de = (math.atan2(end[1] - center[1], end[0] - center[0]) - a0) % tau
    return dm < de


def reverse_el(el: dict) -> dict:
    if el["type"] == "line":
        return {"id": el["id"], "type": "line", "start": el["end"], "end": el["start"]}
    if el["type"] == "arc":
        return {
            "id": el["id"],
            "type": "arc",
            "center": el["center"],
            "start": el["end"],
            "end": el["start"],
            "angle_increasing": not el["angle_increasing"],
        }
    return dict(el)


def signed_area_nm2(elems: list[dict]) -> float:
    shoelace = 0.0
    bulge = 0.0
    tau = 2.0 * math.pi
    for el in elems:
        if el["type"] == "line":
            s, e = el["start"], el["end"]
            shoelace += s[0] * e[1] - e[0] * s[1]
        elif el["type"] == "arc":
            c, s, e, angle_increasing = el["center"], el["start"], el["end"], el["angle_increasing"]
            shoelace += s[0] * e[1] - e[0] * s[1]
            r = math.hypot(s[0] - c[0], s[1] - c[1])
            a0 = math.atan2(s[1] - c[1], s[0] - c[0])
            a1 = math.atan2(e[1] - c[1], e[0] - c[0])
            d = (a1 - a0) % tau
            th = d if angle_increasing else (tau - d) % tau
            if th == 0.0:
                th = tau
            seg = 0.5 * r * r * (th - math.sin(th))
            bulge += seg if angle_increasing else -seg
    return 0.5 * shoelace + bulge


def elem_sort_key(el: dict) -> tuple:
    id_key = (1, el["id"]) if el["id"] is not None else (0, "")
    if el["type"] == "line":
        return (0, el["start"], el["end"], id_key)
    if el["type"] == "arc":
        return (1, el["center"], el["start"], el["end"], 1 if el["angle_increasing"] else 0, id_key)
    return (2, el["center"], el["diameter_nm"], id_key)


def canonicalize_loop(elems: list[dict]) -> list[dict]:
    area = signed_area_nm2(elems)
    oriented = elems if area > 0 else [reverse_el(e) for e in reversed(elems)]
    n = len(oriented)
    rotations = [oriented[k:] + oriented[:k] for k in range(n)]
    return min(rotations, key=lambda rot: [elem_sort_key(e) for e in rot])


def compute_geometry_hash(outline_elems: list[dict], thickness_nm: int) -> str:
    canon = canonicalize_loop(outline_elems)
    h = hashlib.sha256()
    h.update(b"board-interface:v1\0")
    h.update(struct.pack("<q", thickness_nm))
    h.update(struct.pack("<Q", len(canon)))
    for el in canon:
        if el["id"] is None:
            h.update(b"\x00")
        else:
            b = el["id"].encode("utf-8")
            h.update(b"\x01" + struct.pack("<Q", len(b)) + b)
        if el["type"] == "line":
            h.update(b"\x01")
            h.update(struct.pack("<qqqq", el["start"][0], el["start"][1], el["end"][0], el["end"][1]))
        elif el["type"] == "arc":
            h.update(b"\x02")
            h.update(
                struct.pack(
                    "<qqqqqqB",
                    el["center"][0],
                    el["center"][1],
                    el["start"][0],
                    el["start"][1],
                    el["end"][0],
                    el["end"][1],
                    1 if el["angle_increasing"] else 0,
                )
            )
    # Empty cutouts, holes, keepouts
    h.update(struct.pack("<QQQ", 0, 0, 0))
    return "sha256:" + h.hexdigest()


def fmt_pm(pm: int) -> str:
    if pm == 0:
        return "0.000000000\n"
    sign = "-" if pm < 0 else ""
    a = abs(pm)
    return f"{sign}{a // 1_000_000_000}.{a % 1_000_000_000:09d}\n"


def fmt_real(v: float) -> str:
    if v == 0.0 or abs(v) < 0.5e-9:
        v = 0.0
    return f"{v:.9f}\n"


def wrap_deg(deg: float) -> float:
    w = deg % 360.0
    if w >= 360.0 - 0.5e-9:
        return 0.0
    return w


def angle_in_ccw_arc(deg: float, a50: float, a51: float) -> bool:
    if a50 <= a51:
        return a50 <= deg <= a51
    return deg >= a50 or deg <= a51


def write_dxf(outline: list[dict]) -> str:
    min_x, min_y = 10**30, 10**30
    max_x, max_y = -(10**30), -(10**30)

    def inc(x_pm: int, y_pm: int) -> None:
        nonlocal min_x, min_y, max_x, max_y
        min_x = min(min_x, x_pm)
        min_y = min(min_y, y_pm)
        max_x = max(max_x, x_pm)
        max_y = max(max_y, y_pm)

    for el in outline:
        inc(el["start"][0] * 1000, -el["start"][1] * 1000)
        inc(el["end"][0] * 1000, -el["end"][1] * 1000)
        if el["type"] == "arc":
            c, s, e, angle_increasing = el["center"], el["start"], el["end"], el["angle_increasing"]
            dx_s = (s[0] - c[0]) / 1_000_000.0
            dy_s = (c[1] - s[1]) / 1_000_000.0
            dx_e = (e[0] - c[0]) / 1_000_000.0
            dy_e = (c[1] - e[1]) / 1_000_000.0
            r_mm = 0.5 * (math.hypot(dx_s, dy_s) + math.hypot(dx_e, dy_e))
            ang_s = wrap_deg(math.degrees(math.atan2(dy_s, dx_s)))
            ang_e = wrap_deg(math.degrees(math.atan2(dy_e, dx_e)))
            a50, a51 = (ang_e, ang_s) if angle_increasing else (ang_s, ang_e)
            r_pm = round(r_mm * 1_000_000_000.0)
            cx_pm, cy_pm = c[0] * 1000, -c[1] * 1000
            for deg, ox, oy in [(0.0, r_pm, 0), (90.0, 0, r_pm), (180.0, -r_pm, 0), (270.0, 0, -r_pm)]:
                if angle_in_ccw_arc(deg, a50, a51):
                    inc(cx_pm + ox, cy_pm + oy)

    out: list[str] = []
    out.append("  0\nSECTION\n  2\nHEADER\n")
    out.append("  9\n$ACADVER\n  1\nAC1009\n")
    out.append("  9\n$INSBASE\n 10\n" + fmt_pm(0) + " 20\n" + fmt_pm(0) + " 30\n" + fmt_pm(0))
    out.append("  9\n$EXTMIN\n 10\n" + fmt_pm(min_x) + " 20\n" + fmt_pm(min_y) + " 30\n" + fmt_pm(0))
    out.append("  9\n$EXTMAX\n 10\n" + fmt_pm(max_x) + " 20\n" + fmt_pm(max_y) + " 30\n" + fmt_pm(0))
    out.append("  0\nENDSEC\n")

    out.append("  0\nSECTION\n  2\nTABLES\n")
    out.append("  0\nTABLE\n  2\nAPPID\n 70\n     1\n")
    out.append("  0\nAPPID\n  2\nBOARD_INTERFACE\n 70\n     0\n  0\nENDTAB\n")
    out.append("  0\nTABLE\n  2\nLAYER\n 70\n     4\n")
    for name, color in [
        ("BOARD_OUTLINE", 7),
        ("BOARD_CUTOUT", 7),
        ("BOARD_HOLE", 7),
        ("BOARD_KEEPOUT", 8),
    ]:
        out.append(f"  0\nLAYER\n  2\n{name}\n 70\n     0\n 62\n{color:6d}\n  6\nCONTINUOUS\n")
    out.append("  0\nENDTAB\n  0\nENDSEC\n")

    out.append("  0\nSECTION\n  2\nENTITIES\n")
    for el in outline:
        if el["type"] == "line":
            s, e = el["start"], el["end"]
            out.append("  0\nLINE\n  8\nBOARD_OUTLINE\n")
            out.append(" 10\n" + fmt_pm(s[0] * 1000) + " 20\n" + fmt_pm(-s[1] * 1000) + " 30\n" + fmt_pm(0))
            out.append(" 11\n" + fmt_pm(e[0] * 1000) + " 21\n" + fmt_pm(-e[1] * 1000) + " 31\n" + fmt_pm(0))
            out.append("1001\nBOARD_INTERFACE\n1002\n{\n")
            out.append(f"1000\nid\n1000\n{el['id']}\n")
            out.append("1000\nloop\n1071\n     0\n")
            out.append("1002\n}\n")
        elif el["type"] == "arc":
            c, s, e, angle_increasing = el["center"], el["start"], el["end"], el["angle_increasing"]
            dx_s = (s[0] - c[0]) / 1_000_000.0
            dy_s = (c[1] - s[1]) / 1_000_000.0
            dx_e = (e[0] - c[0]) / 1_000_000.0
            dy_e = (c[1] - e[1]) / 1_000_000.0
            r_mm = 0.5 * (math.hypot(dx_s, dy_s) + math.hypot(dx_e, dy_e))
            ang_s = wrap_deg(math.degrees(math.atan2(dy_s, dx_s)))
            ang_e = wrap_deg(math.degrees(math.atan2(dy_e, dx_e)))
            a50, a51 = (ang_e, ang_s) if angle_increasing else (ang_s, ang_e)
            out.append("  0\nARC\n  8\nBOARD_OUTLINE\n")
            out.append(" 10\n" + fmt_pm(c[0] * 1000) + " 20\n" + fmt_pm(-c[1] * 1000) + " 30\n" + fmt_pm(0))
            out.append(" 40\n" + fmt_real(r_mm))
            out.append(" 50\n" + fmt_real(a50))
            out.append(" 51\n" + fmt_real(a51))
            out.append("1001\nBOARD_INTERFACE\n1002\n{\n")
            out.append(f"1000\nid\n1000\n{el['id']}\n")
            out.append("1000\nloop\n1071\n     0\n")
            out.append("1000\nstart_x\n1040\n" + fmt_pm(s[0] * 1000))
            out.append("1000\nstart_y\n1040\n" + fmt_pm(-s[1] * 1000))
            out.append("1000\nend_x\n1040\n" + fmt_pm(e[0] * 1000))
            out.append("1000\nend_y\n1040\n" + fmt_pm(-e[1] * 1000))
            out.append(f"1000\nangle_increasing\n1070\n{1 if angle_increasing else 0:6d}\n")
            out.append("1002\n}\n")
    out.append("  0\nENDSEC\n  0\nEOF\n")
    return "".join(out)


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--pcb", type=Path, default=DEFAULT_PCB, help="Path to easy2350.kicad_pcb")
    parser.add_argument("--out-dir", type=Path, default=DEFAULT_OUT_DIR, help="Output directory")
    args = parser.parse_args()

    root = parse_sexpr(args.pcb.read_text(encoding="utf-8"))
    if not isinstance(root, list) or not root or root[0] != "kicad_pcb":
        raise SystemExit("Input is not a kicad_pcb S-expression")

    general = child_form(root, "general")
    thickness_form = child_form(general, "thickness") if general else None
    if not thickness_form:
        raise SystemExit("Missing (general (thickness ...))")
    thickness_mm = float(thickness_form[1])
    thickness_nm = round(thickness_mm * 1_000_000)

    raw_segs: list[dict] = []
    for item in root[1:]:
        if not isinstance(item, list) or not item:
            continue
        kind = item[0]
        if kind not in ("gr_line", "gr_arc"):
            continue
        layer_form = child_form(item, "layer")
        if not layer_form or layer_form[1] != "Edge.Cuts":
            continue
        uuid_form = child_form(item, "uuid") or child_form(item, "tstamp")
        seg_id = f"kicad:{uuid_form[1]}" if uuid_form else None

        if kind == "gr_line":
            s_form = child_form(item, "start")
            e_form = child_form(item, "end")
            assert s_form and e_form
            start = (mm_to_nm(s_form[1]), mm_to_nm(s_form[2]))
            end = (mm_to_nm(e_form[1]), mm_to_nm(e_form[2]))
            raw_segs.append({"id": seg_id, "type": "line", "start": start, "end": end})
        elif kind == "gr_arc":
            s_form = child_form(item, "start")
            m_form = child_form(item, "mid")
            e_form = child_form(item, "end")
            assert s_form and m_form and e_form
            s_mm = (float(s_form[1]), float(s_form[2]))
            m_mm = (float(m_form[1]), float(m_form[2]))
            e_mm = (float(e_form[1]), float(e_form[2]))
            c_mm = circumcenter_mm(s_mm, m_mm, e_mm)
            angle_increasing = arc_is_ccw_ydown(c_mm, s_mm, m_mm, e_mm)
            raw_segs.append(
                {
                    "id": seg_id,
                    "type": "arc",
                    "center": (round(c_mm[0] * 1_000_000), round(c_mm[1] * 1_000_000)),
                    "start": (mm_to_nm(s_form[1]), mm_to_nm(s_form[2])),
                    "end": (mm_to_nm(e_form[1]), mm_to_nm(e_form[2])),
                    "angle_increasing": angle_increasing,
                }
            )

    # Chain segments into walk order starting from raw_segs[0].
    used = [False] * len(raw_segs)
    walked = [raw_segs[0]]
    used[0] = True
    cur_end = walked[0]["end"]
    for _ in range(1, len(raw_segs)):
        for idx, cand in enumerate(raw_segs):
            if used[idx]:
                continue
            if cand["start"] == cur_end:
                used[idx] = True
                walked.append(cand)
                cur_end = cand["end"]
                break
            if cand["end"] == cur_end:
                used[idx] = True
                rev = reverse_el(cand)
                walked.append(rev)
                cur_end = rev["end"]
                break
        else:
            raise SystemExit(f"Edge.Cuts chain broke at {cur_end}")
    if cur_end != walked[0]["start"]:
        raise SystemExit("Edge.Cuts loop did not close")

    geom_hash = compute_geometry_hash(walked, thickness_nm)
    dxf_text = write_dxf(walked)
    manifest = {
        "schema": 1,
        "board": "easy2350",
        "revision": geom_hash,
        "parent": None,
        "author": {
            "app": "kicad",
            "source": SOURCE_COMMIT,
            "at": PUBLISHED_AT,
        },
        "driver": "kicad",
        "last_handover": None,
        "thickness_mm": thickness_mm,
        "geometry_hash": geom_hash,
        "approximate": False,
    }
    manifest_text = json.dumps(manifest, indent=2) + "\n"

    args.out_dir.mkdir(parents=True, exist_ok=True)
    (args.out_dir / "easy2350.dxf").write_text(dxf_text, encoding="utf-8")
    (args.out_dir / "easy2350.json").write_text(manifest_text, encoding="utf-8")
    print(f"Wrote golden/easy2350.dxf and golden/easy2350.json ({len(walked)} segments, {geom_hash})")


if __name__ == "__main__":
    main()
