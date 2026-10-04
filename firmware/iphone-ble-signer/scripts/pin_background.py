#!/usr/bin/env python3
"""Convert the supplied 128x164 RGBA reference to the 128x160 display asset.

Only the four bottom rows are cropped. Sample digits are removed so firmware
can draw masked, live PIN entry. Standard library only; no image dependencies.
"""
import struct
import zlib
from pathlib import Path

ASSETS = Path(__file__).resolve().parents[1] / "assets"


def convert(source, clear_digits=True):
    assert source[:8] == b"\x89PNG\r\n\x1a\n"
    pos, compressed = 8, bytearray()
    while pos < len(source):
        n = struct.unpack_from(">I", source, pos)[0]
        kind, data = source[pos + 4:pos + 8], source[pos + 8:pos + 8 + n]
        if kind == b"IHDR":
            assert struct.unpack(">IIBBBBB", data) == (128, 164, 8, 6, 0, 0, 0)
        elif kind == b"IDAT":
            compressed.extend(data)
        pos += n + 12
    raw, previous, rgb = zlib.decompress(compressed), bytearray(512), bytearray()
    for y in range(164):
        filt, row = raw[y * 513], bytearray(raw[y * 513 + 1:(y + 1) * 513])
        assert filt in range(5)
        for x in range(512):
            a, b, c = row[x - 4] if x >= 4 else 0, previous[x], previous[x - 4] if x >= 4 else 0
            p = a + b - c
            distances = [abs(p - a), abs(p - b), abs(p - c)]
            paeth = (a, b, c)[distances.index(min(distances))]
            row[x] = (row[x] + (0, a, b, (a + b) // 2, paeth)[filt]) & 255
        previous = row
        if y < 160:
            for x in range(128):
                r, g, b, alpha = row[x * 4:x * 4 + 4]
                assert alpha == 255
                rgb.extend((r, g, b) if y < 90 or not clear_digits else (0, 0, 0))
    return rgb


def png(rgb):
    def chunk(kind, data):
        return struct.pack(">I", len(data)) + kind + data + struct.pack(">I", zlib.crc32(kind + data))
    rows = b"".join(b"\0" + rgb[y * 384:(y + 1) * 384] for y in range(160))
    return b"\x89PNG\r\n\x1a\n" + chunk(b"IHDR", struct.pack(">IIBBBBB", 128, 160, 8, 2, 0, 0, 0)) + chunk(b"IDAT", zlib.compress(rows)) + chunk(b"IEND", b"")


if __name__ == "__main__":
    rgb = convert((ASSETS / "pin_reference.png").read_bytes())
    packed = bytearray()
    for i in range(0, len(rgb), 3):
        r, g, b = rgb[i:i + 3]
        packed.extend(struct.pack("<H", ((r >> 3) << 11) | ((g >> 2) << 5) | (b >> 3)))
    assert len(packed) == 128 * 160 * 2
    (ASSETS / "pin_background_128x160.rgb565").write_bytes(packed)
    (ASSETS / "pin_background_preview.png").write_bytes(png(rgb))
