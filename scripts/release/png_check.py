"""PNG checks for the clean-machine release test (no third-party modules).

  python3 -I png_check.py blank <png>      exit 1 if the image is (nearly) one colour
  python3 -I png_check.py differ <a> <b>   exit 1 if the images are (nearly) equal
"""

import struct
import sys
import zlib


def read_png(path):
    """Returns (width, height, channels, rows of bytes) of an 8-bit RGB(A) PNG."""
    with open(path, "rb") as f:
        data = f.read()
    if data[:8] != b"\x89PNG\r\n\x1a\n":
        raise SystemExit(f"{path}: not a PNG")
    pos, idat, header = 8, b"", None
    while pos < len(data):
        (length,) = struct.unpack(">I", data[pos : pos + 4])
        kind = data[pos + 4 : pos + 8]
        body = data[pos + 8 : pos + 8 + length]
        if kind == b"IHDR":
            header = struct.unpack(">IIBBBBB", body)
        elif kind == b"IDAT":
            idat += body
        pos += 12 + length
    width, height, depth, colour, _, _, interlace = header
    channels = {2: 3, 6: 4}.get(colour)
    if depth != 8 or channels is None or interlace:
        raise SystemExit(f"{path}: unsupported PNG (depth {depth}, colour type {colour})")
    raw = zlib.decompress(idat)
    stride = width * channels
    rows, prev = [], bytearray(stride)
    for y in range(height):
        start = y * (stride + 1)
        kind, line = raw[start], bytearray(raw[start + 1 : start + 1 + stride])
        for i in range(stride):
            a = line[i - channels] if i >= channels else 0
            b = prev[i]
            c = prev[i - channels] if i >= channels else 0
            if kind == 1:
                line[i] = (line[i] + a) & 255
            elif kind == 2:
                line[i] = (line[i] + b) & 255
            elif kind == 3:
                line[i] = (line[i] + (a + b) // 2) & 255
            elif kind == 4:
                p = a + b - c
                pa, pb, pc = abs(p - a), abs(p - b), abs(p - c)
                pred = a if pa <= pb and pa <= pc else (b if pb <= pc else c)
                line[i] = (line[i] + pred) & 255
        rows.append(bytes(line))
        prev = line
    return width, height, channels, rows


def pixels(png, step=4):
    width, height, channels, rows = png
    for y in range(0, height, step):
        row = rows[y]
        for x in range(0, width, step):
            yield row[x * channels : x * channels + 3]


def blank(path):
    png = read_png(path)
    values = list(pixels(png))
    lum = [0.2126 * p[0] + 0.7152 * p[1] + 0.0722 * p[2] for p in values]
    mean = sum(lum) / len(lum)
    std = (sum((v - mean) ** 2 for v in lum) / len(lum)) ** 0.5
    colours = len(set(values))
    print(f"{path}: {png[0]}x{png[1]}, {colours} colours (sampled), luminance mean {mean:.1f} std {std:.1f}")
    return colours < 16 or std < 4.0


def differ(a, b):
    pa, pb = read_png(a), read_png(b)
    if pa[:2] != pb[:2]:
        print(f"{a} and {b} differ in size")
        return True
    changed = total = 0
    for x, y in zip(pixels(pa, 2), pixels(pb, 2)):
        total += 1
        changed += max(abs(i - j) for i, j in zip(x, y)) > 8
    print(f"{a} vs {b}: {100 * changed / total:.2f}% of sampled pixels changed")
    return changed / total > 0.001


if __name__ == "__main__":
    command, files = sys.argv[1], sys.argv[2:]
    if command == "blank":
        sys.exit(1 if blank(files[0]) else 0)
    if command == "differ":
        sys.exit(0 if differ(files[0], files[1]) else 1)
    raise SystemExit(__doc__)
