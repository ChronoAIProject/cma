"""Read complete private detector reports without loading the raw array into memory."""

import json
import re

from .common import ScanError, invalid_json_constant, unique_json_pairs

# Keep a finite aggregate I/O budget and bound the undecoded working buffer to
# a single row. The previous limit covered the whole unredacted report.
MAX_REPORT_BYTES = 512 * 1024 * 1024
MAX_ROW_CHARACTERS = 64 * 1024 * 1024
READ_CHARACTERS = 64 * 1024
WHITESPACE = re.compile(r"[ \t\r\n]*")


def report_rows(path):
    if path.stat().st_size > MAX_REPORT_BYTES:
        raise ScanError("gitleaks-report-too-large")

    decoder = json.JSONDecoder(
        object_pairs_hook=unique_json_pairs, parse_constant=invalid_json_constant
    )
    try:
        with path.open("r", encoding="utf-8-sig", newline="") as stream:
            buffer = ""
            offset = 0

            def refill(count=READ_CHARACTERS):
                nonlocal buffer, offset
                remaining = buffer[offset:]
                block = stream.read(count)
                buffer = remaining + block
                offset = 0
                return bool(block)

            def next_character():
                nonlocal offset
                while True:
                    offset = WHITESPACE.match(buffer, offset).end()
                    if offset < len(buffer):
                        return buffer[offset]
                    if not refill():
                        return ""

            if next_character() != "[":
                raise ScanError("gitleaks-result-inconsistent")
            offset += 1
            if next_character() != "]":
                while True:
                    if next_character() != "{":
                        raise ScanError("invalid-gitleaks-finding")
                    # Retry only with a geometrically increasing prefix, so a
                    # long valid row does not repeatedly decode a short prefix.
                    while True:
                        available = len(buffer) - offset
                        try:
                            row, end = decoder.raw_decode(buffer, offset)
                            if end - offset > MAX_ROW_CHARACTERS:
                                raise ScanError("gitleaks-report-row-too-large")
                            break
                        except json.JSONDecodeError as error:
                            if available > MAX_ROW_CHARACTERS:
                                raise ScanError("gitleaks-report-row-too-large") from error
                            count = min(max(available, READ_CHARACTERS),
                                        MAX_ROW_CHARACTERS + 1 - available)
                            if not refill(count):
                                raise ScanError("invalid-json") from error
                    offset = end
                    yield row
                    delimiter = next_character()
                    if delimiter == "]":
                        break
                    if delimiter != ",":
                        raise ScanError("invalid-json")
                    offset += 1
            offset += 1
            if next_character():
                raise ScanError("invalid-json")
    except (UnicodeError, RecursionError, ValueError) as error:
        raise ScanError("invalid-json") from error
