#!/usr/bin/env python3
"""Generate the original blue book/speaker ICO using only Python's standard library."""
from pathlib import Path
import math
import struct
import zlib

BLUE = (37, 99, 235)
WHITE = (255, 255, 255)
POLYGONS = (
    ((40, 64), (100, 74), (100, 190), (40, 180)),
    ((108, 74), (148, 64), (148, 180), (108, 190)),
    ((160, 112), (176, 112), (196, 96), (196, 160), (176, 144), (160, 144)),
)


def inside_polygon(x, y, points):
    inside = False
    previous = points[-1]
    for current in points:
        ax, ay = previous
        bx, by = current
        if (ay > y) != (by > y) and x < (bx - ax) * (y - ay) / (by - ay) + ax:
            inside = not inside
        previous = current
    return inside


def color(x, y):
    dx, dy = max(abs(x - 128) - 68, 0), max(abs(y - 128) - 68, 0)
    if dx * dx + dy * dy > 52 * 52:
        return (0, 0, 0, 0)
    white = any(inside_polygon(x, y, polygon) for polygon in POLYGONS)
    for radius, angle in ((28, math.pi / 4), (44, math.pi / 4)):
        px, py = x - 190, y - 128
        theta = math.atan2(py, px)
        if abs(theta) <= angle:
            white |= abs(math.hypot(px, py) - radius) <= 4
        for sign in (-1, 1):
            cx, cy = 190 + radius * math.cos(angle), 128 + sign * radius * math.sin(angle)
            white |= (x - cx) ** 2 + (y - cy) ** 2 <= 16
    return (*(WHITE if white else BLUE), 255)


def png(size):
    # Four samples per axis keep the small Explorer and Start menu sizes crisp.
    raw = bytearray()
    for row in range(size):
        raw.append(0)
        for column in range(size):
            samples = [color((column + (sx + 0.5) / 4) * 256 / size,
                             (row + (sy + 0.5) / 4) * 256 / size)
                       for sy in range(4) for sx in range(4)]
            alpha = sum(sample[3] for sample in samples)
            raw.extend(round(sum(sample[channel] * sample[3] for sample in samples) / alpha)
                       if alpha else 0 for channel in range(3))
            raw.append(round(alpha / len(samples)))

    def chunk(kind, data):
        return struct.pack('>I', len(data)) + kind + data + struct.pack('>I', zlib.crc32(kind + data))

    return (b'\x89PNG\r\n\x1a\n'
            + chunk(b'IHDR', struct.pack('>IIBBBBB', size, size, 8, 6, 0, 0, 0))
            + chunk(b'IDAT', zlib.compress(raw, 9)) + chunk(b'IEND', b''))


def main():
    sizes = (16, 20, 24, 32, 40, 48, 64, 128, 256)
    images = [png(size) for size in sizes]
    offset = 6 + 16 * len(images)
    entries = bytearray()
    for size, image in zip(sizes, images):
        entries.extend(struct.pack('<BBBBHHII', size % 256, size % 256, 0, 0,
                                   1, 32, len(image), offset))
        offset += len(image)
    path = Path(__file__).with_name('voice-over-book-reader.ico')
    path.write_bytes(struct.pack('<HHH', 0, 1, len(images)) + entries + b''.join(images))
    print(path)


if __name__ == '__main__':
    main()
