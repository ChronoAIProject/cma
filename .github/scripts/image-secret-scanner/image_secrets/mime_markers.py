"""Bounded MIME magic decoding for the two exactly reviewed generated caches."""

import re
import struct

from .key_vectors import HEADER, sha


def require(condition):
    if not condition:
        raise ValueError("unproved-mime-record")


def marker_value(value):
    matches = list(HEADER.finditer(value))
    return len(matches) == 1 and matches[0].start() == 0 and value[matches[0].end() :] in (b"", b"\n")


def magic_records(data):
    require(data.startswith(b"MIME-Magic\0\n"))
    cursor, mime, records, count = 12, None, [], 0
    while cursor < len(data):
        count += 1
        require(count <= 10000)
        if data[cursor : cursor + 1] == b"[":
            end = data.find(b"\n", cursor, cursor + 300)
            require(end >= 0)
            header = re.fullmatch(rb"\[([0-9]{1,3}):([a-zA-Z0-9.+_/-]+)\]", data[cursor:end])
            require(header is not None)
            mime = header[2].decode("ascii")
            cursor = end + 1
            continue
        prefix = re.match(rb"([0-9]{0,3})>([0-9]{1,10})=", data[cursor : cursor + 32])
        require(mime is not None and prefix is not None)
        indent, start = int(prefix[1] or b"0"), int(prefix[2])
        require(indent <= 64)
        cursor += prefix.end()
        require(cursor + 2 <= len(data))
        size = int.from_bytes(data[cursor : cursor + 2], "big")
        cursor += 2
        offset = cursor
        require(size > 0 and cursor + size <= len(data))
        value = data[cursor : cursor + size]
        cursor += size
        masked = data[cursor : cursor + 1] == b"&"
        if masked:
            cursor += size + 1
        word, span = 1, 1
        for tag in (b"~", b"+"):
            if data[cursor : cursor + 1] == tag:
                number = re.match(rb"[0-9]{1,10}", data[cursor + 1 : cursor + 12])
                require(number is not None)
                if tag == b"~":
                    word = int(number[0])
                else:
                    span = int(number[0])
                cursor += 1 + number.end()
        require(data[cursor : cursor + 1] == b"\n")
        cursor += 1
        if HEADER.search(value):
            require(marker_value(value) and (indent, start, word, span, masked) == (0, 0, 1, 1, False))
            records.append(
                {
                    "offset": offset,
                    "value_length": size,
                    "mime_type": mime,
                    "marker_only_value": True,
                    "record_newline_after_value": True,
                }
            )
    require(cursor == len(data))
    return records


def cache_records(data, expected):
    require(len(data) >= 44 and list(struct.unpack_from(">HH", data)) == expected["format_version"])

    def words(offset, count, lower=0, upper=None):
        require(type(offset) is int and lower <= offset and offset + count * 4 <= (upper or len(data)))
        return struct.unpack_from(">" + "I" * count, data, offset)

    begin, end = words(24, 2)
    require(44 <= begin < end <= len(data))
    count, _extent, table = words(begin, 3)
    require(count == expected["magic_entries"] and count <= 10000 and table == begin + 12)
    matchlets = table + 16 * count
    require(matchlets <= end)
    pending, visited, payloads, records = [], set(), [], []
    for i in range(count):
        _priority, type_offset, matches, offset = words(table + i * 16, 4, table, matchlets)
        require(44 <= type_offset < begin)
        stop = data.find(b"\0", type_offset, min(type_offset + 256, begin))
        require(stop > type_offset)
        mime = data[type_offset:stop].decode("ascii")
        require(re.fullmatch(r"[a-zA-Z0-9.+_/-]+", mime) is not None)
        pending.append((matches, offset, mime, 0))
    while pending:
        count, offset, mime, depth = pending.pop()
        require(count <= 10000 and depth <= 64 and len(visited) + count <= 10000)
        for i in range(count):
            position = offset + i * 32
            require(position not in visited and (position - matchlets) % 32 == 0)
            visited.add(position)
            start, span, word, size, value_at, mask, children, child_at = words(position, 8, matchlets, end)
            require(size <= 1024 * 1024 and matchlets <= value_at and value_at + size <= end)
            payloads.append((value_at, size))
            if mask:
                require(matchlets <= mask and mask + size <= end)
                payloads.append((mask, size))
            if children:
                pending.append((children, child_at, mime, depth + 1))
            else:
                require(child_at == 0)
            value = data[value_at : value_at + size]
            if HEADER.search(value):
                require(marker_value(value) and (start, span, word, mask, children) == (0, 1, 1, 0, 0))
                records.append(
                    {
                        "offset": value_at,
                        "matchlet_offset": position,
                        "value_length": size,
                        "mime_type": mime,
                        "start_offset": start,
                        "range_length": span,
                        "word_size": word,
                        "mask_offset": mask,
                        "children": children,
                        "marker_only_value": True,
                    }
                )
    require(sorted(visited) == list(range(matchlets, matchlets + 32 * len(visited), 32)))
    cursor = matchlets + 32 * len(visited)
    for offset, size in sorted(payloads):
        require(offset == cursor)
        cursor = (offset + size + 3) & ~3
        require(cursor <= end and not any(data[offset + size : cursor]))
    require(cursor == end)
    return sorted(records, key=lambda item: item["offset"])


def mime_matches(data, state, structure):
    inputs = structure["inputs"]
    # An extra generator input makes the stored provenance incomplete even when
    # the cache happens to retain its old bytes. Missing/linked inputs also fail.
    expected_paths = {item["path"] for item in inputs}
    actual_paths = {path for path in state if path.startswith("usr/share/mime/packages/")}
    if expected_paths != actual_paths:
        return False
    for item in [*inputs, structure["generator"]]:
        content = state.get(item["path"], b"")
        if sha(content) != item["sha256"] or ("bytes" in item and len(content) != item["bytes"]):
            return False
    try:
        records = magic_records(data) if structure["format"] == "magic" else cache_records(data, structure)
    except (ValueError, UnicodeError, IndexError, struct.error):
        return False
    return records == structure["records"]
