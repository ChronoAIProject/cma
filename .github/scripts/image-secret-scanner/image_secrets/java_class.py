"""Read the bounded JVM constant-pool string spans used for source attribution."""

MAGIC = b"\xca\xfe\xba\xbe"
MAX_CLASS_BYTES = 16 * 1024 * 1024
FIXED_BYTES = {
    3: 4, 4: 4, 5: 8, 6: 8, 7: 2, 8: 2, 9: 4, 10: 4,
    11: 4, 12: 4, 15: 3, 16: 2, 17: 4, 18: 4, 19: 2, 20: 2,
}


def utf8_spans(data):
    """Read complete CONSTANT_Utf8_info entries without loading or executing classes."""
    if not 10 <= len(data) <= MAX_CLASS_BYTES or data[:4] != MAGIC:
        return set()
    entries = int.from_bytes(data[8:10], "big")
    if entries < 2:
        return set()
    offset = 10
    index = 1
    spans = set()
    while index < entries:
        if offset >= len(data):
            return set()
        tag = data[offset]
        offset += 1
        if tag == 1:
            if offset + 2 > len(data):
                return set()
            size = int.from_bytes(data[offset:offset + 2], "big")
            offset += 2
            spans.add((offset, offset + size))
        elif tag in FIXED_BYTES:
            size = FIXED_BYTES[tag]
        else:
            return set()
        offset += size
        index += 2 if tag in (5, 6) else 1
    # The class still needs access, class, and the four collection fields.
    return spans if index == entries and offset + 14 <= len(data) else set()
