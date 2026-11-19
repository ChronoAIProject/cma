"""Recognize private material in bounded OpenPGP key streams (RFC 9580)."""

from .common import ScanError

PRIVATE_ARMOR = b"-----BEGIN PGP PRIVATE KEY BLOCK-----"
SECRET_TAGS = frozenset({5, 7})
PUBLIC_TAGS = frozenset({6, 14})
KEY_TAGS = PUBLIC_TAGS | SECRET_TAGS
KEY_VERSIONS = frozenset({2, 3, 4, 5, 6})
MARKER_TAG = 10
MARKER = b"PGP"
NON_CRITICAL_TAG_MIN = 40
MAX_PACKETS = 100_000


def length(stream, count):
    data = stream.read(count)
    if len(data) != count:
        return None
    return int.from_bytes(data, "big")


def packet(stream):
    header = length(stream, 1)
    if header is None or header & 0x80 == 0:
        return None, None
    if header & 0x40:
        tag = header & 63
        size = length(stream, 1)
        if size is None or size < 192:
            return tag, size
        if size <= 223:
            extra = length(stream, 1)
            return tag, None if extra is None else ((size - 192) << 8) + extra + 192
        return tag, length(stream, 4) if size == 255 else None
    tag, encoding = (header >> 2) & 15, header & 3
    return tag, length(stream, 1 << encoding) if encoding != 3 else None


def secret_key_stream(path):
    end = path.stat().st_size
    with path.open("rb") as stream:
        return secret_key_packets(stream, end)


def secret_key_packets(stream, end):
    recognized = False
    for _ in range(MAX_PACKETS):
        if stream.tell() == end:
            return False
        tag, size = packet(stream)
        start = stream.tell()
        if tag == MARKER_TAG:
            # RFC 9580 permits markers before and between key packets.
            if size != len(MARKER) or stream.read(size) != MARKER:
                return recognized
            continue
        if (not recognized and tag is not None and tag >= NON_CRITICAL_TAG_MIN
                and size is not None and start + size <= end):
            # RFC 9580 also permits unknown non-critical packets before
            # the key. Their bounded bodies do not establish a key type.
            stream.seek(start + size)
            continue
        if not recognized:
            if tag not in KEY_TAGS or length(stream, 1) not in KEY_VERSIONS:
                return False
            recognized = True
            if size == 0:
                return True
        if tag in SECRET_TAGS or tag is None or size is None or start + size > end:
            return True
        stream.seek(start + size)
    raise ScanError("openpgp-packet-budget-exceeded")
