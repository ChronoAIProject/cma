"""Expose ELF strings to the text detector without executing binary content."""

from .common import digest_file

TEXT_BYTES = bytes(value if value in (9, 10, 13) or 32 <= value < 127 else 10 for value in range(256))


def prepare(physical, budget):
    with physical.open("rb") as stream:
        if stream.read(4) != b"\x7fELF":
            return None
    # Gitleaks' directory reader skips ELF. Raw-byte policy checks and nested
    # identification have already run; preserve all ASCII bytes for its rules.
    # Bind detections to the original file, never the temporary text view.
    original = digest_file(physical)
    budget.reserve(physical.stat().st_size)
    view = physical.with_suffix(".view")
    try:
        with physical.open("rb") as source, view.open("xb") as output:
            for block in iter(lambda: source.read(65536), b""):
                output.write(block.translate(TEXT_BYTES))
        view.replace(physical)
    finally:
        view.unlink(missing_ok=True)
    return original
