#!/usr/bin/env python3
"""Validate an ESP image and append its optional integrity digest for espflash 3.3."""
import argparse
import hashlib
from pathlib import Path
import struct


def with_integrity_digest(image):
    if len(image) < 24 or image[0] != 0xE9 or not 1 <= image[1] <= 16 or image[23] not in (0, 1):
        raise ValueError("invalid ESP image header")
    position, checksum = 24, 0xEF
    for _ in range(image[1]):
        if position + 8 > len(image):
            raise ValueError("truncated segment header")
        _, length = struct.unpack_from("<II", image, position)
        position += 8
        if position + length > len(image):
            raise ValueError("truncated segment data")
        for byte in image[position:position + length]:
            checksum ^= byte
        position += length
    checksum_position = position + (15 - position % 16)
    if checksum_position >= len(image) or image[checksum_position] != checksum:
        raise ValueError("ESP image checksum mismatch")
    body_end = checksum_position + 1
    expected_length = body_end + (32 if image[23] else 0)
    if len(image) != expected_length:
        raise ValueError("unexpected ESP image padding/length")
    if image[23]:
        if hashlib.sha256(image[:body_end]).digest() != image[body_end:]:
            raise ValueError("ESP image integrity digest mismatch")
        return image
    # espflash 3.3 assumes the last 32 bytes are a digest and rewrites them.
    # Without this trailer it overwrites executable code and the XOR checksum.
    body = bytearray(image)
    body[23] = 1
    return bytes(body) + hashlib.sha256(body).digest()


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("source", type=Path)
    parser.add_argument("destination", type=Path)
    args = parser.parse_args()
    if args.source.resolve() == args.destination.resolve():
        parser.error("keep the generated SDK bootloader unchanged; use a separate output")
    args.destination.write_bytes(with_integrity_digest(args.source.read_bytes()))


if __name__ == "__main__":
    main()
