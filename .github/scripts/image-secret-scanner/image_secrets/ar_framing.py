"""Bound physical ar headers before native name decoding; never extract paths."""

import stat

from .common import ScanError

METADATA = {b"/": "symbol-table", b"//": "name-table", b"/SYM64/": "symbol-table-64"}
MAX_METADATA_BYTES = 16 * 1024 * 1024
MAX_NAME_BYTES = 16384


def require(condition):
    if not condition:
        raise ScanError("ar-framing-invalid")


def decimal(field):
    field = field.strip(b" ")
    require(bool(field) and all(48 <= char <= 57 for char in field))
    return int(field)


def bsd_name(stream, offset, prefix):
    stream.seek(offset)
    raw = stream.read(prefix)
    require(len(raw) == prefix)
    name, _, padding = raw.partition(b"\0")
    # BSD permits NUL alignment padding, not hidden bytes that the native
    # C-string filename interface would silently discard.
    require(bool(name) and not any(padding))
    return name


def frames(path, byte_limit, member_limit, entry_limit):
    result = []
    total = path.stat().st_size
    with path.open("rb") as stream:
        require(stream.read(8) == b"!<arch>\n")
        while stream.tell() < total:
            if len(result) >= entry_limit:
                raise ScanError("entry-budget-exceeded")
            header = stream.read(60)
            require(len(header) == 60 and header[58:] == b"`\n")
            name = header[:16].rstrip(b" ")
            require(bool(name))
            size = decimal(header[48:58])
            if size > member_limit:
                raise ScanError("nested-member-size-limit")
            if size > byte_limit:
                raise ScanError("byte-budget-exceeded")
            byte_limit -= size
            mode = header[40:48].strip(b" ")
            if not mode and name in METADATA:
                mode = b"0"
            require(bool(mode) and all(48 <= char <= 55 for char in mode))
            require(stat.S_IFMT(int(mode, 8)) in (0, stat.S_IFREG))
            if name in METADATA:
                require(size <= MAX_METADATA_BYTES)
            prefix = decimal(name[3:]) if name.startswith(b"#1/") else 0
            require(prefix <= min(size, MAX_NAME_BYTES))
            offset = stream.tell()
            require(offset + size + size % 2 <= total)
            if name.startswith(b"#1/"):
                bsd_name(stream, offset, prefix)
            result.append((name, offset, size, prefix))
            stream.seek(offset + size)
            if size % 2:
                require(stream.read(1) == b"\n")
        require(stream.tell() == total)
    return result
