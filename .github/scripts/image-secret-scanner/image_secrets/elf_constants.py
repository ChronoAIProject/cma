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
        p_type, flags, offset, address, _, file_size, memory_size = struct.unpack_from(
            "<IIQQQQQ", data, table + i * width)
        if p_type != 1:
            continue
        if file_size > memory_size or offset + file_size > len(data) or address + memory_size > 2**64:
            return ()
        if memory_size:
            segments.append((flags, offset, address, file_size, memory_size))
    return segments


def address_of(segments, start, length, allowed):
    end = start + length
    occupied = [segment for segment in segments if segment[3] and max(start, segment[1]) < min(end, segment[1] + segment[3])]
    if len(occupied) != 1:
        return None
    (flags, offset, virtual, file_size, _), = occupied
    if flags not in allowed or not offset <= start < end <= offset + file_size:
        return None
    address = virtual + start - offset
    mappings = [entry for entry in segments if max(address, entry[2]) < min(address + length, entry[2] + entry[4])]
    return address if mappings == occupied else None


def word_pair(data, segments, reference, address, length):
    site = reference["offset"]
    return (site % 8 == 0 and address_of(segments, site, 16, (4, 6)) is not None
            and digest_bytes(data[site:site + 16]) == reference["sha256"]
            and struct.unpack_from("<QQ", data, site) == (address, length))


def rip_load(lea, virtual):
    rex, opcode, operand = lea[:3]
    if rex not in (0x48, 0x4C) or opcode != 0x8D or operand & 0xC7 != 0x05:
        return None
    register = (operand >> 3 & 7) + (8 if rex == 0x4C else 0)
    displacement, = struct.unpack_from("<i", lea, 3)
    return register, virtual + 7 + displacement


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
    loaded = rip_load(lea, base + loc - earlier)
    if loaded is None or loaded[1] != address:
        return False
    destination = loaded[0]
    if destination not in GO_ARGUMENT_REGISTERS[:-1]:
        return False
    length_reg = GO_ARGUMENT_REGISTERS[GO_ARGUMENT_REGISTERS.index(destination) + 1]
    if len(mov) == 6 and mov[0] != 0x41:
        return False
    actual_reg = mov[-5] - 0xB8 + (8 if len(mov) == 6 else 0)
    return 0xB8 <= mov[-5] <= 0xBF and actual_reg == length_reg and int.from_bytes(mov[-4:], "little") == length


def stack_pair(data, segments, reference, address, length):
    code = reference["code"]
    start = code["offset"]
    virtual = address_of(segments, start, 21, (5,))
    body = data[start:start + 21]
    if virtual is None or digest_bytes(body) != code["sha256"]:
        return False
    loaded = rip_load(body[:7], virtual)
    pointer, count = body[7:12], body[12:]
    if (loaded is None or loaded[1] != address or pointer[0] not in (0x48, 0x4C) or pointer[1] != 0x89
            or pointer[2] & 0xC7 != 0x44 or pointer[3] != 0x24 or count[:4] != b"\x48\xc7\x44\x24"):
        return False
    register = (pointer[2] >> 3 & 7) + (8 if pointer[0] == 0x4C else 0)
    return (register == loaded[0] and pointer[4] + 8 == count[4] < 128
            and int.from_bytes(count[5:], "little") == length)
