"""Renders the Recto icon: a glowing page with its corner folded over, and a few lines
of text, on a dark warm tile. Gold to orange, so it sits with Earshot's warm tones but
stays apart from its coral and plum (and from Glance's blue and Kerf's violet).

Writes recto.ico (16-256 px, PNG-compressed entries) and recto.png (256 px, used for
the window icon). Needs only numpy.

    python assets/make_icon.py
"""
import struct
import zlib
from pathlib import Path

import numpy as np

OUT = Path(__file__).parent


def hexrgb(h):
    return np.array([int(h[i:i + 2], 16) / 255 for i in (1, 3, 5)])


TOP, BOTTOM = hexrgb("#3b2312"), hexrgb("#120905")
GOLD, ORANGE = hexrgb("#ffd56b"), hexrgb("#ff6f2e")
# The page outline (x, y; y points down), its folded corner, and the text lines.
L, T, R, B, FOLD = 0.29, 0.20, 0.71, 0.80, 0.14
OUTLINE = [(R - FOLD, T), (L, T), (L, B), (R, B), (R, T + FOLD), (R - FOLD, T)]
FLAP = [(R - FOLD, T), (R - FOLD, T + FOLD), (R, T + FOLD)]
LINES = [(0.42, 0.58), (0.52, 0.62), (0.62, 0.62), (0.72, 0.52)]
LINES_SMALL = [(0.47, 0.60), (0.63, 0.56)]


def seg_dist(x, y, a, b):
    """Distance from every pixel to the segment a-b."""
    ax, ay = a
    bx, by = b
    ex, ey = bx - ax, by - ay
    t = np.clip(((x - ax) * ex + (y - ay) * ey) / (ex * ex + ey * ey), 0, 1)
    return np.hypot(x - ax - t * ex, y - ay - t * ey)


def render(size, ss=4):
    n = size * ss
    y, x = (np.mgrid[0:n, 0:n] + 0.5) / n
    # Rounded square tile.
    r, m = 0.22, 0.04
    dx = np.maximum(np.abs(x - 0.5) - (0.5 - m - r), 0)
    dy = np.maximum(np.abs(y - 0.5) - (0.5 - m - r), 0)
    tile = np.clip((r - np.hypot(dx, dy)) * n, 0, 1)
    color = TOP + (BOTTOM - TOP) * y[..., None]
    color = color + (ORANGE - color) * (np.exp(-np.hypot(x - 0.5, y - 0.5) * 5) * 0.22)[..., None]

    small = size <= 24
    w = 0.05 if small else 0.028  # stroke half-width
    d = np.full(x.shape, np.inf)
    for path in (OUTLINE, FLAP):
        for a, b in zip(path, path[1:]):
            d = np.minimum(d, seg_dist(x, y, a, b) - w)
    for ly, x1 in (LINES_SMALL if small else LINES):
        d = np.minimum(d, seg_dist(x, y, (L + 0.09, ly), (x1, ly)) - w * 0.9)
    # A faint fill inside the page, so it reads as a sheet.
    inside = (x > L) & (x < R) & (y > T) & (y < B) & ((R - x) + (y - T) > FOLD)
    color = color + (GOLD - color) * (inside * 0.08)[..., None]

    # Gold at the top left, orange towards the bottom right.
    t = np.clip(((x - L) + (y - T)) / ((R - L) + (B - T)), 0, 1)
    ink = GOLD + (ORANGE - GOLD) * t[..., None]
    glow = np.exp(-np.maximum(d, 0) * 22)[..., None] * 0.45
    color = color + (ink - color) * glow
    color = color + (ink - color) * np.clip(-d * n, 0, 1)[..., None]
    rgba = np.dstack([np.clip(color, 0, 1), tile])
    rgba = rgba.reshape(size, ss, size, ss, 4).mean(axis=(1, 3))
    return (rgba * 255 + 0.5).astype(np.uint8)


def png(img):
    h, w, _ = img.shape
    raw = b"".join(b"\0" + img[i].tobytes() for i in range(h))

    def chunk(t, d):
        return struct.pack(">I", len(d)) + t + d + struct.pack(">I", zlib.crc32(t + d) & 0xFFFFFFFF)

    return (b"\x89PNG\r\n\x1a\n" + chunk(b"IHDR", struct.pack(">IIBBBBB", w, h, 8, 6, 0, 0, 0))
            + chunk(b"IDAT", zlib.compress(raw, 9)) + chunk(b"IEND", b""))


sizes = [16, 24, 32, 48, 64, 128, 256]
images = [png(render(s)) for s in sizes]
header = struct.pack("<HHH", 0, 1, len(sizes))
offset = 6 + 16 * len(sizes)
entries = b""
for s, data in zip(sizes, images):
    entries += struct.pack("<BBBBHHII", s % 256, s % 256, 0, 0, 1, 32, len(data), offset)
    offset += len(data)
(OUT / "recto.ico").write_bytes(header + entries + b"".join(images))
(OUT / "recto.png").write_bytes(images[-1])
print("wrote recto.ico, recto.png")
