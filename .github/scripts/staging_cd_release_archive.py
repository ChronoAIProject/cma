"""The OCI archive handoff inside a trusted staging release job."""

import json
import re
import stat
from datetime import date
from pathlib import Path
from typing import NamedTuple

from image_secret_gate import checksum
from staging_cd_common import Refusal, require
from staging_cd_receipts import canonical, validate_identity

SCHEMA = "cma-staging-build-archive-v2"
LEGACY_SCHEMA = "cma-staging-build-archive-v1"
ARCHIVE = "image.oci.tar"
MANIFEST = "archive.json"
MAX_MANIFEST_BYTES = 4096


class ProducedArchive(NamedTuple):
    path: Path
    sha256: str
    cache_namespace: str | None
    cache_day: date | None


def regular_file(path):
    entry = path.lstat()
    require(stat.S_ISREG(entry.st_mode), "Release archive handoff contains a nonregular file")
    return entry


def seal(directory, identity, cache_namespace=None, cache_day=None):
    validate_identity(identity)
    require((cache_namespace is None and cache_day is None)
            or (identity["component"] in ("am", "eg") and type(cache_day) is date
                and isinstance(cache_namespace, str) and re.fullmatch(r"[0-9a-f]{32}", cache_namespace)),
            "Release archive cache identity is malformed")
    archive = directory / ARCHIVE
    size = regular_file(archive).st_size
    require(size > 0, "Release archive handoff is empty")
    document = {
        "schema": SCHEMA,
        "identity": identity,
        "archive_bytes": size,
        "archive_sha256": checksum(archive),
        "cache_namespace": cache_namespace,
        "cache_day": cache_day.isoformat() if cache_day is not None else None,
    }
    with (directory / MANIFEST).open("x", encoding="utf-8") as stream:
        stream.write(canonical(document) + "\n")


def load(directory, expected):
    """An absent handoff allows building. A present corrupt handoff refuses."""
    require(not directory.parent.is_symlink(), "Release archive handoff parent is invalid")
    if not directory.exists() and not directory.is_symlink():
        return None
    require(directory.is_dir() and not directory.is_symlink(), "Release archive handoff path is invalid")
    manifest = directory / MANIFEST
    require(regular_file(manifest).st_size <= MAX_MANIFEST_BYTES, "Release archive handoff identity is oversized")
    document = json.loads(manifest.read_text(encoding="utf-8"))
    fields = {"schema", "identity", "archive_bytes", "archive_sha256", "cache_namespace"}
    require(isinstance(document, dict)
            and ((document.get("schema") == SCHEMA and set(document) == fields | {"cache_day"})
                 or (document.get("schema") == LEGACY_SCHEMA and set(document) == fields)),
            "Release archive handoff identity is malformed")
    require(validate_identity(document["identity"]) == expected, "Release archive handoff identity differs")
    namespace = document["cache_namespace"]
    require(namespace is None or isinstance(namespace, str) and re.fullmatch(r"[0-9a-f]{32}", namespace),
            "Release archive cache namespace is malformed")
    cache_day = document.get("cache_day")
    if document["schema"] == SCHEMA:
        require((namespace is None and cache_day is None)
                or (namespace is not None and expected["component"] in ("am", "eg")
                    and isinstance(cache_day, str) and re.fullmatch(r"\d{4}-\d{2}-\d{2}", cache_day)),
                "Release archive cache identity is malformed")
    try:
        day = date.fromisoformat(cache_day) if cache_day is not None else None
    except ValueError as error:
        raise Refusal("Release archive cache identity is malformed") from error
    archive = directory / ARCHIVE
    size = regular_file(archive).st_size
    require(type(document["archive_bytes"]) is int and size > 0 and size == document["archive_bytes"]
            and checksum(archive) == document["archive_sha256"], "Release archive handoff bytes differ")
    return ProducedArchive(archive, document["archive_sha256"], namespace, day)


def path_for(root, reservation):
    return root / (reservation["component"] + "-" + reservation["version"])
