"""Verify bounded AMD64 ELF references to immutable string bytes."""

import struct

from .common import digest_bytes

GO_ARGUMENT_REGISTERS = (0, 3, 1, 7, 6, 8, 9, 10, 11)


def load_segments(data):
    if len(data) < 64 or data[:7] != b"\x7fELF\x02\x01\x01":
        return ()
    kind, machine = struct.unpack_from("<HH", data, 16)
    table, = struct.unpack_from("<Q", data, 32)
    width, count = struct.unpack_from("<HH", data, 54)
    if (kind not in (2, 3) or machine != 62 or width != 56 or not 0 < count <= 128
            or table < 64 or table + width * count > len(data)):
        return ()
    segments = []
    for i in range(count):
        p_type, flags, offset, address, _, size = struct.unpack_from("<IIQQQQ", data, table + i * width)
        if p_type == 1 and size:
            if offset + size > len(data) or address + size > 2**64:
                return ()
            segments.append((flags, offset, address, size))
    return segments


def address_of(segments, start, length, allowed):
    occupied = [(flags, virtual + start - offset) for flags, offset, virtual, size in segments
                if offset <= start < start + length <= offset + size]
    if len(occupied) != 1:
        return None
    (flags, address), = occupied
    return address if flags in allowed else None


def word_pair(data, segments, reference, address, length):
    site = reference["offset"]
    return (site % 8 == 0 and address_of(segments, site, 16, (4, 6)) is not None
            and digest_bytes(data[site:site + 16]) == reference["sha256"]
            and struct.unpack_from("<QQ", data, site) == (address, length))


def instruction_pair(data, segments, reference, address, length):
    left, right = reference["address"], reference["length"]
    loc, count = left["offset"], right["offset"]
    earlier, later = min(loc, count), max(loc + 7, count + right["size"])
    base = address_of(segments, earlier, later - earlier, (5,))
    if base is None or not (loc + 7 == count or count + right["size"] == loc):
        return False
    lea = data[loc:loc + 7]
    mov = data[count:count + right["size"]]
    if digest_bytes(lea) != left["sha256"] or digest_bytes(mov) != right["sha256"]:
        return False
    rex, opcode, operand = lea[:3]
    if rex not in (0x48, 0x4C) or opcode != 0x8D or operand & 0xC7 != 0x05:
        return False
    destination = (operand >> 3 & 7) + (8 if rex == 0x4C else 0)
    if destination not in GO_ARGUMENT_REGISTERS[:-1]:
        return False
    length_reg = GO_ARGUMENT_REGISTERS[GO_ARGUMENT_REGISTERS.index(destination) + 1]
    if len(mov) == 6 and mov[0] != 0x41:
        return False
    actual_reg = mov[-5] - 0xB8 + (8 if len(mov) == 6 else 0)
    displacement, = struct.unpack_from("<i", lea, 3)
    return (0xB8 <= mov[-5] <= 0xBF and actual_reg == length_reg and int.from_bytes(mov[-4:], "little") == length
            and base + loc - earlier + 7 + displacement == address)
