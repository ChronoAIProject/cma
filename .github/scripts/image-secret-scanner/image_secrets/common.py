"""Small shared contracts; error messages are identifiers, never input content."""

import hashlib
import json
from pathlib import PurePosixPath

MAX_NESTED_MEMBER_BYTES = 256 * 1024 * 1024


class ScanError(Exception):
    """A safe, fixed error identifier suitable for a public report."""


def digest_file(path):
    digest = hashlib.sha256()
    with path.open("rb") as stream:
        for block in iter(lambda: stream.read(1024 * 1024), b""):
            digest.update(block)
    return digest.hexdigest()


def digest_bytes(value):
    return hashlib.sha256(value).hexdigest()


def archive_path(name, *, posix_tar=False):
    # Tar paths exist only in the virtual POSIX image. Host staging paths
    # and detector filenames are assigned independently.
    if not isinstance(name, str) or "\x00" in name or ("\\" in name and not posix_tar):
        raise ScanError("unsafe-archive-path")
    path = PurePosixPath(name)
    if path.is_absolute() or ".." in path.parts:
        raise ScanError("unsafe-archive-path")
    return str(path)


def tar_metadata(path, member, *, ascii_only=False):
    # tarfile represents binary PAX values with surrogate escapes. Retain those
    # bytes as JSON escapes while keeping ordinary UTF-8 text visible to detectors.
    data = {"path": path, "link": member.linkname, "pax": member.pax_headers}
    return json.dumps(data, ensure_ascii=ascii_only).encode("utf-8", errors="backslashreplace")


def unique_json_pairs(items):
    # Duplicate JSON keys make descriptor/config interpretation ambiguous.
    result = {}
    for key, value in items:
        if key in result:
            raise ScanError("duplicate-json-key")
        result[key] = value
    return result


def invalid_json_constant(value):
    raise ScanError("nonstandard-json-constant")


def json_bytes(data):
    try:
        return json.loads(data, object_pairs_hook=unique_json_pairs, parse_constant=invalid_json_constant)
    except (ValueError, UnicodeError, RecursionError) as error:
        raise ScanError("invalid-json") from error


def write_json(path, value):
    with path.open("x", encoding="utf-8") as stream:
        json.dump(value, stream, indent=2, ensure_ascii=True)
        stream.write("\n")


class Budget:
    """A hard cap fails the scan rather than silently omitting large entries."""

    def __init__(self, max_bytes, max_entries):
        self.max_bytes = max_bytes
        self.max_entries = max_entries
        self.bytes = 0
        self.entries = 0

    def entry(self):
        self.entries += 1
        if self.entries > self.max_entries:
            raise ScanError("entry-budget-exceeded")

    def reserve(self, count):
        if count < 0 or self.bytes + count > self.max_bytes:
            raise ScanError("byte-budget-exceeded")
        self.bytes += count


def copy_stream(source, destination, limit):
    total = 0
    with destination.open("xb") as output:
        for block in iter(lambda: source.read(1024 * 1024), b""):
            total += len(block)
            if total > limit:
                raise ScanError("expanded-layer-budget-exceeded")
            output.write(block)
    return total
