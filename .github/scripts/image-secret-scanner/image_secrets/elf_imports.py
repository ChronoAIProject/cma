"""Verify a bounded AMD64 ELF import through its dynamic relocation table."""

import struct

from . import elf_constants

MAX_RELOCATION_BYTES = 16 * 1024 * 1024
REQUIRED = frozenset({5, 6, 7, 8, 9, 10, 11})
PLT = frozenset({2, 20, 23})
OTHER_RELOCATIONS = frozenset({17, 18, 19, 35, 36, 37})
RELATIVE_COUNT = 0x6FFFFFF9  # DT_RELACOUNT
READABLE = (4, 6)
EIGHT_BYTE_RELOCATIONS = frozenset({6, 7, 8})  # GLOB_DAT, JUMP_SLOT, RELATIVE


def dynamic_tables(data, segments):
    dynamic = [header for header in elf_constants.program_headers(data) if header[0] == 2]
    if len(dynamic) != 1:
        return {}
    _, _, start, address, _, size, _ = dynamic[0]
    if not 0 < size <= 64 * 1024 or size % 16 or elf_constants.address_of(segments, start, size, READABLE) != address:
        return {}
    values = {}
    for i in range(start, start + size, 16):
        tag, value = struct.unpack_from("<qQ", data, i)
        if tag == 0:
            return values if REQUIRED <= values.keys() and (values.keys() & PLT) in (set(), PLT) else {}
        if tag in OTHER_RELOCATIONS:
            return {}
        if tag in REQUIRED | PLT | {RELATIVE_COUNT}:
            if tag in values:
                return {}
            values[tag] = value
    return {}


def has_import(data, segments, slot, symbol_name):
    values = dynamic_tables(data, segments)
    if not values or values[9] != 24 or values[11] != 24:
        return False
    if elf_constants.file_offset(segments, slot, 8, (6,)) is None:
        return False
    tables = [(values[7], values[8], values.get(RELATIVE_COUNT, 0))]
    if 23 in values:
        if values[20] != 7:  # DT_PLTREL must name DT_RELA.
            return False
        tables.append((values[23], values[2], 0))
    selected = []
    for address, size, relative in tables:
        if not 0 < size <= MAX_RELOCATION_BYTES or size % 24 or relative > size // 24:
            return False
        relocation = elf_constants.file_offset(segments, address, size, READABLE)
        if relocation is None:
            return False
        for i in range(relocation, relocation + size, 24):
            offset, info, addend = struct.unpack_from("<QQq", data, i)
            kind = info & 0xFFFFFFFF
            if kind not in EIGHT_BYTE_RELOCATIONS or offset + 8 > 2**64:
                return False
            if max(offset, slot) < min(offset + 8, slot + 8):
                if offset != slot or i < relocation + 24 * relative:
                    return False
                selected.append((info >> 32, kind, addend))
                if len(selected) > 1:
                    return False
    if len(selected) != 1:
        return False
    index, kind, addend = selected[0]
    if not index or kind != 6 or addend != 0:  # R_X86_64_GLOB_DAT
        return False
    symbol = elf_constants.file_offset(segments, values[6] + 24 * index, 24, READABLE)
    if symbol is None:
        return False
    name, info, _, section, value, _ = struct.unpack_from("<IBBHQQ", data, symbol)
    if section != 0 or value != 0 or info not in (0x12, 0x22) or name + len(symbol_name) + 1 > values[10]:
        return False
    offset = elf_constants.file_offset(segments, values[5] + name, len(symbol_name) + 1, READABLE)
    return offset is not None and data[offset:offset + len(symbol_name) + 1] == symbol_name + b"\0"
