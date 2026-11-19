"""Bind ZIP local records and descriptors to the bounded central directory."""

import struct

from .common import ScanError

MAX_PREFIX_BYTES = 65536
# These fields describe size, timestamps or Unix IDs. Other extension fields
# could change the path or type an installer assigns to the decoded bytes.
REGULAR_PROOF_EXTRA_TYPES = frozenset({0x0001, 0x5455, 0x7875})


def require(condition):
    if not condition:
        raise ScanError("nested-zip-framing-invalid")


def member_ends(members, directory_start):
    offsets = sorted(member.header_offset for member in members)
    require(len(set(offsets)) == len(offsets))
    require(not offsets or 0 <= offsets[0] <= offsets[-1] < directory_start)
    return dict(zip(offsets, offsets[1:] + [directory_start]))


def leading_prefix(path, end):
    # A directory cannot reclassify omitted physical members as an arbitrary
    # self-extractor prefix. Support only bounded plain-text preambles, whose
    # exact bytes are submitted separately to the detector by the caller.
    if not 0 <= end <= MAX_PREFIX_BYTES:
        raise ScanError("nested-zip-prefix-size-limit")
    with path.open("rb") as stream:
        prefix = stream.read(end)
    require(len(prefix) == end)
    if any(byte not in (9, 10, 13) and not 32 <= byte <= 126 for byte in prefix):
        raise ScanError("nested-zip-unexplained-prefix")
    return prefix


def extra_fields(extra):
    while extra:
        require(len(extra) >= 4)
        kind, length = struct.unpack("<HH", extra[:4])
        require(4 + length <= len(extra))
        data, extra = extra[4:4 + length], extra[4 + length:]
        yield kind, data


def local_sizes(extra, size, compressed):
    zip64 = size == 0xffffffff or compressed == 0xffffffff
    found = False
    for kind, data in extra_fields(extra):
        if kind == 1:
            require(not found)
            found = True
            if size == 0xffffffff:
                require(len(data) >= 8)
                size, data = struct.unpack("<Q", data[:8])[0], data[8:]
            if compressed == 0xffffffff:
                require(len(data) >= 8)
                compressed = struct.unpack("<Q", data[:8])[0]
    require(not zip64 or found)
    return size, compressed


def payload_extent(stream, member, end):
    stream.seek(member.header_offset)
    header = stream.read(30)
    require(len(header) == 30)
    signature, _, flags, method, _, _, crc, compressed, size, names, extras = struct.unpack(
        "<4s5H3L2H", header)
    require(signature == b"PK\x03\x04" and flags == member.flag_bits and method == member.compress_type)
    # Only stored/deflate and their ordinary UTF-8/data-descriptor flags are
    # supported; encryption and unimplemented ZIP codecs fail closed.
    require(method in (0, 8) and flags & ~0x080e == 0)
    raw_name, extra = stream.read(names), stream.read(extras)
    require(len(raw_name) == names and len(extra) == extras and b"\0" not in raw_name)
    require(raw_name.decode("utf-8" if flags & 0x800 else "cp437") == member.orig_filename)
    size, compressed = local_sizes(extra, size, compressed)
    start = stream.tell()
    stop = start + member.compress_size
    require(start <= stop <= end)
    if flags & 8:
        require(crc in (0, member.CRC) and size in (0, member.file_size)
                and compressed in (0, member.compress_size))
        length = end - stop
        # Some streaming ZIP64 writers use zero local sizes without sentinel
        # values. Bind the exact physical descriptor extent and all fields;
        # do not guess its width from those untrusted local size placeholders.
        require(length in (12, 16, 20, 24))
        stream.seek(stop)
        descriptor = stream.read(length)
        require(len(descriptor) == length)
        if length in (16, 24):
            require(descriptor[:4] == b"PK\x07\x08")
            descriptor = descriptor[4:]
        require(struct.unpack("<LQQ" if len(descriptor) == 20 else "<LLL", descriptor)
                == (member.CRC, member.compress_size, member.file_size))
    else:
        require((crc, compressed, size) == (member.CRC, member.compress_size, member.file_size))
        require(stop == end)
    stream.seek(start)
    local_proof = all(kind in REGULAR_PROOF_EXTRA_TYPES for kind, _ in extra_fields(extra))
    central_proof = all(kind in REGULAR_PROOF_EXTRA_TYPES for kind, _ in extra_fields(member.extra))
    return member.compress_size, local_proof and central_proof
