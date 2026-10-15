#!/usr/bin/env python3
"""Draw the Mini Remote app icon (no extra libraries needed) and build AppIcon.icns.

A rounded blue square with a white screen and a mouse pointer.
Usage: python3 scripts/macos/make-icon.py   (writes assets/AppIcon.icns)
"""

import math
import os
import shutil
import struct
import subprocess
import sys
import tempfile
import zlib

SIZE = 1024
ROOT = os.path.abspath(os.path.join(os.path.dirname(__file__), "..", ".."))


def rounded_rect_sdf(px, py, cx, cy, hw, hh, r):
    """Signed distance to a rounded rectangle (negative inside)."""
    qx = abs(px - cx) - hw + r
    qy = abs(py - cy) - hh + r
    outside = math.hypot(max(qx, 0.0), max(qy, 0.0))
    inside = min(max(qx, qy), 0.0)
    return outside + inside - r


def coverage(d):
    """Anti-aliased coverage from a signed distance."""
    return max(0.0, min(1.0, 0.5 - d))


def polygon_sdf(px, py, pts):
    """Signed distance to a polygon (negative inside)."""
    d = float("inf")
    inside = False
    n = len(pts)
    for i in range(n):
        ax, ay = pts[i]
        bx, by = pts[i - 1]
        ex, ey = bx - ax, by - ay
        wx, wy = px - ax, py - ay
        h = max(0.0, min(1.0, (wx * ex + wy * ey) / (ex * ex + ey * ey)))
        d = min(d, math.hypot(wx - ex * h, wy - ey * h))
        if (ay > py) != (by > py) and px < (bx - ax) * (py - ay) / (by - ay) + ax:
            inside = not inside
    return -d if inside else d


def blend(dst, src, a):
    return tuple(d * (1 - a) + s * a for d, s in zip(dst, src))


def render():
    top = (99, 102, 241)      # indigo
    bottom = (37, 99, 235)    # blue
    white = (255, 255, 255)
    screen_fill = (224, 231, 255)
    rows = []
    # Classic arrow pointer (tip top-left) over the lower right of the screen.
    shape = [(0, 0), (0, 300), (70, 235), (120, 345), (170, 322), (122, 216), (212, 212)]
    ox, oy = 570, 430
    arrow = [(ox + x, oy + y) for x, y in shape]
    for y in range(SIZE):
        row = bytearray()
        t = y / SIZE
        bg = tuple(top[i] * (1 - t) + bottom[i] * t for i in range(3))
        for x in range(SIZE):
            px, py = x + 0.5, y + 0.5
            # Icon body: Apple's grid uses an 824 px body inside 1024.
            body = coverage(rounded_rect_sdf(px, py, 512, 512, 412, 412, 186))
            if body <= 0:
                row += b"\0\0\0\0"
                continue
            col = bg
            # Monitor outline and screen.
            outer = coverage(rounded_rect_sdf(px, py, 512, 450, 290, 200, 44))
            inner = coverage(rounded_rect_sdf(px, py, 512, 450, 250, 160, 18))
            col = blend(col, white, outer)
            col = blend(col, screen_fill, inner)
            # Stand.
            stand = coverage(rounded_rect_sdf(px, py, 512, 690, 40, 40, 8))
            base = coverage(rounded_rect_sdf(px, py, 512, 738, 130, 18, 18))
            col = blend(col, white, max(stand, base))
            # Pointer with a dark outline.
            d = polygon_sdf(px, py, arrow)
            col = blend(col, (30, 41, 59), coverage(d - 14))
            col = blend(col, white, coverage(d))
            row += bytes(int(round(c)) for c in col) + bytes([int(round(body * 255))])
        rows.append(bytes(row))
    return rows


def write_png(path, rows, size):
    raw = b"".join(b"\0" + r for r in rows)
    def chunk(tag, data):
        c = tag + data
        return struct.pack(">I", len(data)) + c + struct.pack(">I", zlib.crc32(c) & 0xFFFFFFFF)
    png = b"\x89PNG\r\n\x1a\n"
    png += chunk(b"IHDR", struct.pack(">IIBBBBB", size, size, 8, 6, 0, 0, 0))
    png += chunk(b"IDAT", zlib.compress(raw, 9))
    png += chunk(b"IEND", b"")
    with open(path, "wb") as f:
        f.write(png)


def main():
    out_dir = os.path.join(ROOT, "assets")
    os.makedirs(out_dir, exist_ok=True)
    master = os.path.join(out_dir, "icon-1024.png")
    print("drawing icon...")
    write_png(master, render(), SIZE)
    if sys.platform != "darwin":
        print("wrote", master)
        return
    tmp = tempfile.mkdtemp()
    iconset = os.path.join(tmp, "AppIcon.iconset")
    os.makedirs(iconset)
    for base in (16, 32, 128, 256, 512):
        for scale in (1, 2):
            px = base * scale
            name = f"icon_{base}x{base}{'@2x' if scale == 2 else ''}.png"
            subprocess.run(["sips", "-z", str(px), str(px), master, "--out", os.path.join(iconset, name)],
                           check=True, capture_output=True)
    subprocess.run(["iconutil", "-c", "icns", iconset, "-o", os.path.join(out_dir, "AppIcon.icns")], check=True)
    shutil.rmtree(tmp)
    print("wrote", os.path.join(out_dir, "AppIcon.icns"))


if __name__ == "__main__":
    main()
