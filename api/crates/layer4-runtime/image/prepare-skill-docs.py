"""Prepare credential examples in the pinned runtime skill payload."""
import hashlib
import re
import sys
from pathlib import Path

DEVICES_SHA256 = "17c8045ec28413eeb8f116750658cf3f2277e5e0af665be38f02955634d07adf"
CHANNELS_SHA256 = "6f01675cea4f139f4ded49803667ec41e094e9692fd422179e263d27e7a6a236"
REFRESH_FIELD = re.compile(rb'("refresh_token"\s*:\s*")[0-9a-fA-F]{64}(")')
PUSH_AUTH_HEADER = re.compile(rb'(?m)^([ \t]+-H "Authorization: Bearer )[^"\r\n]+(" \\)$')
TOKEN_FIELD = re.compile(rb"""(?ix) (?<![a-z0-9_-]) [a-z0-9_-]*token [`"'*_]*
                               (?: \s* [:=] \s* | [ \t]* \| [ \t]* )
                               [`"'*_]* [0-9a-f]{64} (?![a-z0-9]) """)
TOKEN_COLUMN = re.compile(rb"[a-z0-9_-]*token", re.IGNORECASE)
HEX_VALUE = re.compile(rb"[0-9a-f]{64}", re.IGNORECASE)
TABLE_PUNCTUATION = re.compile(rb"\\.|\|")


def table_cells(line):
    line = line.strip()
    cells, start = [], 0
    # Consume an escape and its following byte together. Thus only an
    # unescaped pipe separates columns, even after a run of backslashes.
    for mark in TABLE_PUNCTUATION.finditer(line):
        if mark[0] == b"|":
            cells.append(line[start:mark.start()])
            start = mark.end()
    if not cells:
        return []
    cells.append(line[start:])
    # One optional pipe frames each edge. Further empty cells have positions.
    if cells[0] == b"":
        cells.pop(0)
    if cells[-1] == b"":
        cells.pop()
    return [cell.strip().strip(b"`*_\"'").strip() for cell in cells]


def literal_token(data):
    if TOKEN_FIELD.search(data):
        return True
    # A Markdown table can also name the field only in its header. Keep the
    # column positions so a checksum next to a token stays a checksum.
    previous, columns = [], []
    for line in data.splitlines():
        if b"|" not in line:
            previous, columns = [], []
            continue
        cells = table_cells(line)
        if len(previous) == len(cells) and all(re.fullmatch(rb":?-+:?", cell) for cell in cells):
            columns = [i for i, name in enumerate(previous) if TOKEN_COLUMN.fullmatch(name)]
        elif any(i < len(cells) and HEX_VALUE.fullmatch(cells[i]) for i in columns):
            return True
        previous = cells
    return False


def replace_example(path, expected, pattern, replacement):
    if path.resolve(strict=True) != path:
        raise ValueError("Unexpected NyxID documentation target")
    original = path.read_bytes()
    if hashlib.sha256(original).hexdigest() != expected:
        raise ValueError("Review changed NyxID documentation before packaging")
    updated, count = pattern.subn(replacement, original)
    if count != 1:
        raise ValueError("Expected exactly one NyxID credential example")
    return updated


def prepare(skills):
    root = Path(skills).resolve(strict=True)
    references = root / "nyxid/references"
    devices = references / "devices.md"
    channels = references / "channels.md"
    device_docs = replace_example(devices, DEVICES_SHA256, REFRESH_FIELD, rb'\g<1><refresh_token>\g<2>')
    channel_docs = replace_example(
        channels, CHANNELS_SHA256, PUSH_AUTH_HEADER,
        rb'\g<1>${NYXID_AGENT_API_KEY:?Set NYXID_AGENT_API_KEY}\g<2>',
    )
    devices.write_bytes(device_docs)
    channels.write_bytes(channel_docs)
    # Inspect all regular skill inputs, including extensionless documentation.
    # No file name, field value, or neighboring text enters a build log.
    for path in root.rglob("*"):
        if not path.resolve(strict=True).is_relative_to(root):
            raise ValueError("Skill entry points outside its payload")
        if path.is_file() and literal_token(path.read_bytes()):
            raise ValueError("A packaged skill contains a literal token field")


if __name__ == "__main__":
    prepare(sys.argv[1])
    print("Packaged skill credential examples verified")
