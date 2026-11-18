"""Recognize prefixed ZIPs consistently across Python versions; zipfile decodes them."""

import struct

END = b"PK\x05\x06"
DIRECTORY = b"PK\x01\x02"
ZIP64_LOCATOR = b"PK\x06\x07"
END_SIZE = 22
MAX_COMMENT = 65535


def prefixed_zip(path):
    # Python 3.9 is_zipfile accepts an incidental EOCD marker in bytecode. Require
    # directory evidence, as newer CPython does, without using private stdlib APIs.
    size = path.stat().st_size
    with path.open("rb") as stream:
        start = max(0, size - END_SIZE - MAX_COMMENT)
        stream.seek(start)
        tail = stream.read()
        offset = tail.rfind(END)
        while offset >= 0:
            if directory_evidence(stream, tail, offset, start):
                return True
            offset = tail.rfind(END, 0, offset)
        return False


def directory_evidence(stream, tail, offset, start):
    if offset + END_SIZE > len(tail):
        return False
    # ZIP comments can themselves contain EOCD bytes; examine preceding candidates.
    _, disk, cd_disk, disk_entries, entries, cd_size, cd_offset, comment = struct.unpack(
        "<4s4H2LH", tail[offset : offset + END_SIZE]
    )
    end = start + offset
    if offset + END_SIZE + comment > len(tail):
        return False
    # ZIP64's locator immediately precedes EOCD. Let zipfile validate/decode
    # the extended records; malformed or unsupported layouts must fail closed.
    if end >= 20:
        stream.seek(end - 20)
        if stream.read(4) == ZIP64_LOCATOR:
            return True
    if entries == disk_entries == cd_size == cd_offset == disk == cd_disk == 0:
        return True
    # For SFX archives, stored offsets may exclude the executable prefix.
    actual_directory = end - cd_size
    if entries and cd_size >= 46 and 0 <= cd_offset <= actual_directory < end:
        stream.seek(actual_directory)
        if stream.read(4) == DIRECTORY:
            return True
    # Executables that implement ZIP handling contain all three signatures.
    # Require structurally bounded file records before treating a damaged EOCD as
    # archive evidence; raw bytes still pass through every content detector.
    return file_records(tail[:offset])


def file_records(data):
    central = data.find(DIRECTORY)
    while central >= 0:
        if central + 46 <= len(data):
            name_size, extra_size, comment_size = struct.unpack_from("<3H", data, central + 28)
            end = central + 46 + name_size + extra_size + comment_size
            if name_size and end <= len(data):
                local = data.find(b"PK\x03\x04", 0, central)
                while local >= 0:
                    if local + 30 <= central:
                        local_name_size, local_extra_size = struct.unpack_from("<2H", data, local + 26)
                        payload = local + 30 + local_name_size + local_extra_size
                        # Disagreeing names are corruption, not grounds to skip
                        # decoding a recognizable container.
                        if local_name_size and payload <= central:
                            return True
                    local = data.find(b"PK\x03\x04", local + 1, central)
        central = data.find(DIRECTORY, central + 1)
    return False
