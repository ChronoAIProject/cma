"""Recognize private material in bounded OpenPGP key streams (RFC 9580)."""

from .common import ScanError

PRIVATE_ARMOR = b"-----BEGIN PGP PRIVATE KEY BLOCK-----"
SECRET_TAGS = frozenset({5, 7})
PUBLIC_TAGS = frozenset({6, 14})
KEY_TAGS = PUBLIC_TAGS | SECRET_TAGS
KEY_VERSIONS = frozenset({2, 3, 4, 5, 6})
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
    # Key rings begin with a key packet. Distinguish them from unrelated
    # binaries before walking the other packets in the same stream.
    end = path.stat().st_size
    with path.open("rb") as stream:
        tag, size = packet(stream)
        if tag not in KEY_TAGS:
            return False
        start = stream.tell()
        if length(stream, 1) not in KEY_VERSIONS:
            return False
        if tag in SECRET_TAGS:
            return True
        for _ in range(MAX_PACKETS):
            if size is None or size < 1 or start + size > end:
                return True  # Recognizable key stream with ambiguous framing.
            if start + size == end:
                return False
            stream.seek(start + size)
            tag, size = packet(stream)
            if tag in SECRET_TAGS:
                return True
            if tag is None:
                return True
            start = stream.tell()
    raise ScanError("openpgp-packet-budget-exceeded")
