"""Bounded physical ar inventory; filtered tool listings cannot prove coverage."""

import re
from dataclasses import dataclass

from .common import ScanError, archive_path

MAX_METADATA = 16 * 1024 * 1024
BSD_INDEXES = {"__.SYMDEF", "__.SYMDEF SORTED", "__.SYMDEF_64", "__.SYMDEF_64 SORTED"}


def require(condition):
    if not condition:
        raise ScanError("invalid-ar-structure")


def decimal(value):
    require(re.fullmatch(rb"[0-9]+", value) is not None)
    return int(value)


def safe_name(value):
    try:
        name = value.decode("utf-8")
    except UnicodeError as error:
        raise ScanError("invalid-ar-member-name") from error
    require(
        bool(name)
        and len(value) <= 4096
        and not name.startswith("-")
        and not any(ord(char) < 32 or ord(char) == 127 for char in name)
    )
    return archive_path(name)


@dataclass
class Member:
    offset: int
    start: int
    size: int
    name: str
    metadata: bool = False
    fragments: tuple = ()


def read_metadata(stream, member):
    require(member.size <= MAX_METADATA)
    stream.seek(member.start)
    value = stream.read(member.size)
    require(len(value) == member.size)
    return value


def strings(data):
    """Consume every symbol byte; embedded controls/compressed payloads refuse."""
    names = {}
    offset = 0
    while offset < len(data):
        end = data.find(b"\0", offset)
        require(end > offset)
        value = data[offset:end]
        require(len(value) <= 65536 and all(byte >= 32 and byte != 127 for byte in value))
        names[offset] = value
        offset = end + 1
    return names


def gnu_symbols(data, width, offsets):
    require(len(data) >= width)
    count = int.from_bytes(data[:width], "big")
    end = width * (count + 1)
    require(count <= len(data) // width and end <= len(data))
    for cursor in range(width, end, width):
        require(int.from_bytes(data[cursor : cursor + width], "big") in offsets)
    payload = data[end:]
    # GNU ar may include one NUL inside an even-sized member to align names.
    if payload.endswith(b"\0\0") and len(data) % 2 == 0:
        payload = payload[:-1]
    names = strings(payload)
    require(len(names) == count)
    return tuple(names.values())


def bsd_symbols(data, width, offsets):
    valid = []
    for order in ("little", "big"):
        try:
            require(len(data) >= width * 2)
            length = int.from_bytes(data[:width], order)
            end = width + length
            require(length % (width * 2) == 0 and end + width <= len(data))
            size = int.from_bytes(data[end : end + width], order)
            remaining = data[end + width :]
            # BSD ranlib pads its string table to a machine-word boundary.
            require(size <= len(remaining) and len(remaining) - size < width and not any(remaining[size:]))
            names = strings(remaining[:size])
            used = set()
            for cursor in range(width, end, width * 2):
                name = int.from_bytes(data[cursor : cursor + width], order)
                target = int.from_bytes(data[cursor + width : cursor + width * 2], order)
                require(name in names and target in offsets)
                used.add(name)
            require(used == set(names))
            valid.append(tuple(names.values()))
        except ScanError:
            continue
    require(bool(valid) and all(item == valid[0] for item in valid))
    return valid[0]


def inventory(source, limit):
    result = []
    length = source.stat().st_size
    with source.open("rb") as stream:
        require(stream.read(8) == b"!<arch>\n")
        while stream.tell() < length:
            require(len(result) < limit)
            offset = stream.tell()
            header = stream.read(60)
            require(len(header) == 60 and header[58:] == b"`\n")
            for field, pattern in (
                (header[16:28], rb"[0-9]*"),
                (header[28:34], rb"[0-9]*"),
                (header[34:40], rb"[0-9]*"),
                (header[40:48], rb"[0-7]*"),
            ):
                require(re.fullmatch(pattern, field.strip(b" ")) is not None)
            size = decimal(header[48:58].strip(b" "))
            start = stream.tell()
            require(size <= length - start)
            encoded = header[:16].rstrip(b" ")
            data_start, data_size = start, size
            if encoded.startswith(b"#1/"):
                name_size = decimal(encoded[3:])
                require(0 < name_size <= min(size, 4096))
                encoded = stream.read(name_size)
                # Extended names may be NUL-padded, never contain a hidden tail.
                encoded = encoded.rstrip(b"\0")
                require(b"\0" not in encoded)
                data_start += name_size
                data_size -= name_size
            try:
                name = encoded.decode("ascii")
            except UnicodeError:
                name = safe_name(encoded)
            result.append(Member(offset, data_start, data_size, name))
            stream.seek(start + size)
            if size % 2:
                require(stream.read(1) == b"\n")
        require(stream.tell() == length)
        tables = [member for member in result if member.name == "//"]
        require(len(tables) <= 1)
        long_names = {}
        if tables:
            data = read_metadata(stream, tables[0])
            cursor = 0
            while cursor < len(data):
                if cursor == len(data) - 1 and data[cursor:] == b"\n" and len(data) % 2 == 0:
                    break
                end = data.find(b"/\n", cursor)
                require(end > cursor)
                long_names[cursor] = safe_name(data[cursor:end])
                cursor = end + 2
            tables[0].fragments = tuple(name.encode() for name in long_names.values())
        seen = set()
        for member in result:
            name = member.name
            member.metadata = name in {"/", "/SYM64/", "//"} or name.rstrip("/") in BSD_INDEXES
            if member.metadata:
                normalized = name.rstrip("/") if name.rstrip("/") in BSD_INDEXES else name
            elif name.startswith("/"):
                index = decimal(name[1:].encode())
                require(index in long_names)
                normalized = long_names[index]
            else:
                normalized = safe_name(name.removesuffix("/").encode())
            require(normalized not in seen)
            seen.add(normalized)
            member.name = normalized
        indexes = [member for member in result if member.metadata and member.name != "//"]
        require(len(indexes) <= 1)
        offsets = {member.offset for member in result if not member.metadata}
        for member in indexes:
            data = read_metadata(stream, member)
            if member.name in {"/", "/SYM64/"}:
                member.fragments = gnu_symbols(data, 8 if member.name == "/SYM64/" else 4, offsets)
            else:
                member.fragments = bsd_symbols(data, 8 if "_64" in member.name else 4, offsets)
    return result
