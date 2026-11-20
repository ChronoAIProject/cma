"""Verify package ownership against the current layer and reserved build input."""

import re
from pathlib import Path, PurePosixPath

from .common import archive_path, digest_bytes, digest_file
from .gate import valid_digest

STATUS_PATH = "var/lib/dpkg/status"
STATUS_ANCESTORS = {str(parent) for parent in PurePosixPath(STATUS_PATH).parents}
STATUS_LIMIT = 16 * 1024 * 1024
PRODUCER_ROOT = "api/crates/layer4-runtime/"
INPUT_LIMIT = 4 * 1024 * 1024
NATIVE_FIELDS = ("name", "version", "architecture")
CONTROL_FIELDS = (b"Package", b"Version", b"Architecture", b"Status")
CONTROL_PARAGRAPH = re.compile(rb"\n[ \t]*\n")


def dpkg_identity(package):
    return tuple(package[field] for field in NATIVE_FIELDS)


def installed_versions(data):
    """Index unique installed package versions, independent of field order."""
    versions = {}
    for paragraph in CONTROL_PARAGRAPH.split(data):
        fields = {}
        for line in paragraph.splitlines():
            key, separator, value = line.partition(b":")
            if separator and key in CONTROL_FIELDS:
                if key in fields:
                    return {}
                fields[key] = value.strip().decode("ascii", errors="replace")
        if fields.get(b"Status") != "install ok installed":
            continue
        if not all(fields.get(key) for key in CONTROL_FIELDS):
            return {}
        name, version, architecture = (fields[key] for key in CONTROL_FIELDS[:-1])
        identity = (name, architecture)
        if identity in versions:
            return {}
        versions[identity] = version
    return versions


def status_entry(layer, member, budget):
    if not member.isfile() or member.issparse() or not 0 < member.size <= STATUS_LIMIT:
        return None
    budget.reserve(member.size)
    with layer.extractfile(member) as source:
        raw = source.read(member.size + 1)
    if len(raw) != member.size:
        return None
    return digest_bytes(raw), installed_versions(raw)


def hides_status(path, member):
    location = PurePosixPath(path)
    if location.name == ".wh..wh..opq":
        return str(location.parent) in STATUS_ANCESTORS
    if location.name.startswith(".wh."):
        hidden = str(location.parent / location.name[4:])
        return hidden == STATUS_PATH or hidden in STATUS_ANCESTORS
    opaque = any("overlay." in key for key in member.pax_headers)
    return (path in STATUS_ANCESTORS and (not member.isdir() or opaque)) or (path == STATUS_PATH and opaque)


def check_reserved_inputs(source, expected):
    if source is None:
        return False
    root = Path(source).resolve()
    for name, digest in expected.items():
        target = root / name
        if (target.resolve() != target or not target.is_file() or target.stat().st_size > INPUT_LIMIT
                or digest_file(target) != digest):
            return False
    return True


def valid_producer_inputs(paths):
    return (isinstance(paths, dict) and 0 < len(paths) <= 16
            and all(isinstance(name, str) and name.startswith(PRODUCER_ROOT)
                    and len(name) < 512 and archive_path(name) == name and valid_digest(digest)
                    for name, digest in paths.items()))
