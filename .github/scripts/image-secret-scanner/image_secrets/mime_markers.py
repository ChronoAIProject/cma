"""Locate public key-format discriminators in generated Shared MIME-info data."""

import re
import struct

from .openpgp import PRIVATE_ARMOR

MAX_INDEX_BYTES = 8 * 1024 * 1024
MAGIC_HEADER = b"MIME-Magic\0\n"
MIME_TYPE = b"application/pgp-keys"
ARMOR_BEGIN = b"-----BEGIN "
DISCRIMINATORS = {
    MIME_TYPE: PRIVATE_ARMOR,
    b"application/pkcs8": ARMOR_BEGIN + b"PRIVATE KEY-----",
    b"application/pkcs8-encrypted": ARMOR_BEGIN + b"ENCRYPTED PRIVATE KEY-----",
    b"text/x-ssh-private-key": ARMOR_BEGIN + b"OPENSSH PRIVATE KEY-----",
}
SECTION = re.compile(rb"\[[0-9]{1,3}:([A-Za-z0-9!#$&^_.+/-]{1,127})\]\n")
MATCH = re.compile(rb"([0-9]{0,7})>([0-9]{1,10})=")
FLAGS = re.compile(rb"(?:~[0-9]{1,10})?(?:\+[0-9]{1,10})?\n")


def marker_ranges(data, path):
    if not 0 < len(data) <= MAX_INDEX_BYTES:
        return set()
    if path == "usr/share/mime/magic":
        return magic_ranges(data)
    if path == "usr/share/mime/mime.cache":
        return cache_ranges(data)
    return set()


def magic_ranges(data):
    if data[:len(MAGIC_HEADER)] != MAGIC_HEADER:
        return set()
    cursor = len(MAGIC_HEADER)
    current_type = None
    ranges = set()
    while cursor < len(data):
        if (section := SECTION.match(data, cursor)) is not None:
            current_type = section[1]
            cursor = section.end()
            continue
        match = MATCH.match(data, cursor)
        if current_type is None or match is None or match.end() + 2 > len(data):
            return set()
        start = match.end() + 2
        size = int.from_bytes(data[match.end():start], "big")
        end = start + size
        if not size or end >= len(data):
            return set()
        tail = end + (size + 1 if data[end:end + 1] == b"&" else 0)
        flags = FLAGS.match(data, tail)
        if flags is None:
            return set()
        if (match[1] in (b"", b"0") and match[2] == b"0" and tail == end
                and flags[0] == b"\n" and data[start:end] == DISCRIMINATORS.get(current_type)):
            ranges.add((start, end))
        cursor = flags.end()
    return ranges


def cache_ranges(data):
    # The major/minor header and list layouts use network byte order.
    if len(data) < 40 or data[:4] != b"\0\x01\0\x02":
        return set()
    magic, = struct.unpack_from(">I", data, 24)
    if not 40 <= magic <= len(data) - 12 or magic % 4:
        return set()
    count, _, first = struct.unpack_from(">III", data, magic)
    if not magic + 12 <= first <= len(data) or first % 4 or count > (len(data) - first) // 16:
        return set()
    ranges = set()
    for cursor in range(first, first + 16 * count, 16):
        _, mime_offset, matches, start = struct.unpack_from(">IIII", data, cursor)
        if not 40 <= mime_offset < len(data):
            return set()
        mime_end = data.find(b"\0", mime_offset, mime_offset + 128)
        discriminator = DISCRIMINATORS.get(data[mime_offset:mime_end]) if mime_end != -1 else None
        if discriminator is None:
            continue
        if not first + 16 * count <= start <= len(data) or start % 4 or matches > (len(data) - start) // 32:
            return set()
        for record in range(start, start + 32 * matches, 32):
            offset, width, word, size, value, mask, children, _ = struct.unpack_from(">8I", data, record)
            if not 40 <= value <= len(data) or size > len(data) - value:
                return set()
            if (offset == 0 and width == word == 1 and not mask and not children
                    and data[value:value + size] == discriminator):
                ranges.add((value, value + size))
    return ranges
