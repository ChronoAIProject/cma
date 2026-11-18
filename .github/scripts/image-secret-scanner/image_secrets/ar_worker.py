"""Private ar decoding worker: bounded frames on stdout, no filesystem extraction."""

import json
import struct
import sys
from pathlib import Path

from .ar_framing import METADATA, frames, require
from .ar_native import ArReader
from .common import ScanError, archive_path


def raw_chunks(stream, offset, size):
    stream.seek(offset)
    while size:
        block = stream.read(min(size, 65536))
        require(bool(block))
        size -= len(block)
        yield block


def decode(path, byte_limit, member_limit, entry_limit, output):
    records = frames(path, byte_limit, member_limit, entry_limit)
    reader = ArReader(path)
    try:
        with path.open("rb") as stream:
            for ordinal, (raw_name, offset, physical_size, prefix) in enumerate(records):
                entry = reader.header()
                require(entry is not None)
                name, size = entry
                if raw_name in METADATA:
                    require(name.encode() == raw_name)
                    # libarchive consumes GNU name-table bytes internally. Scan
                    # their exact physical payload too, rather than dropping it.
                    name = "@ar-metadata/" + METADATA[raw_name]
                    size = physical_size
                    chunks = raw_chunks(stream, offset, size)
                else:
                    require(size == physical_size - prefix)
                    name = archive_path(name)
                    require(name not in ("", "."))
                    chunks = reader.chunks(size)
                header = json.dumps({"ordinal": ordinal, "path": name, "size": size}).encode()
                require(len(header) <= 65536)
                output.write(struct.pack(">Q", len(header)))
                output.write(header)
                for block in chunks:
                    output.write(block)
            require(reader.header() is None)
        output.write(bytes(8))
        output.flush()
    finally:
        reader.close()


def main():
    try:
        path, byte_limit, member_limit, entry_limit = sys.argv[1:]
        limits = [int(value) for value in (byte_limit, member_limit, entry_limit)]
        require(all(value >= 0 for value in limits))
        decode(Path(path), *limits, sys.stdout.buffer)
    except (OSError, ValueError, TypeError, AttributeError, ScanError):
        # Native/parser errors may contain member names or input bytes. The
        # parent reports a fixed failure code and never forwards diagnostics.
        return 2
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
