"""Prove image occurrences using the same bounded layer and member readers as detection."""

import hashlib
import tarfile
import tempfile
from collections import defaultdict
from pathlib import Path

from .archive import ImageArchive
from .common import Budget, MAX_NESTED_MEMBER_BYTES, archive_path, copy_stream, digest_file
from .containers import inspect_nested
from .package_evidence import NESTED_LOCATOR, PLATFORM, matches, normalized, require
from .tar_trailer import verify_trailer

IDENTITY_BUDGET = 20 * 1024**3
DECODER_TIMEOUT = 120


def platform(image):
    config = image["config"]
    parts = [config.get("os"), config.get("architecture")]
    if "variant" in config:
        parts.append(config["variant"])
    if not all(isinstance(part, str) for part in parts):
        return None
    value = "/".join(parts)
    return value if matches(PLATFORM, value) else None


def stream_digest(stream, size):
    checksum = hashlib.sha256()
    actual = 0
    for block in iter(lambda: stream.read(1024 * 1024), b""):
        checksum.update(block)
        actual += len(block)
    require(actual == size)
    return checksum.hexdigest()


class Members:
    timeout = DECODER_TIMEOUT

    def __init__(self, workspace, budget, requested):
        self.workspace = workspace
        self.budget = budget
        self.requested = requested
        self.matched = {}

    def add(self, data, _location):
        self.budget.reserve(len(data))

    def inspect_path(self, _location, _path):
        # A payload finding cannot be proved from path or link metadata.
        return None

    def file(self, stream, size, location, _original_path, depth, *, regular=True):
        self.budget.reserve(size)
        require(size <= MAX_NESTED_MEMBER_BYTES)
        locator = location["member"]
        has_child = any(name.startswith(locator + "/") for name in self.requested)
        if not regular or (locator not in self.requested and not has_child):
            # ar streams members over one pipe; each extent must be consumed
            # even when the requested proof belongs to a later member.
            stream_digest(stream, size)
            return
        with tempfile.TemporaryDirectory(dir=self.workspace) as child:
            stored = Path(child) / "member"
            require(copy_stream(stream, stored, size) == size)
            if locator in self.requested:
                self.matched[locator] = (location["path"], digest_file(stored))
            if has_child:
                inspect_nested(self, stored, location, depth)


def prove_package_member(artifact, artifact_sha256, locator, workspace):
    """Derive member bytes from an artifact whose digest was authenticated externally."""
    require(matches(NESTED_LOCATOR, locator))
    with tempfile.TemporaryDirectory(prefix="trusted-package-", dir=workspace) as temp:
        scratch = Path(temp)
        protected = scratch / "artifact"
        with artifact.open("rb") as stream:
            copy_stream(stream, protected, MAX_NESTED_MEMBER_BYTES)
        require(digest_file(protected) == artifact_sha256)
        stage = Members(scratch, Budget(IDENTITY_BUDGET, 1_000_000), {locator})
        inspect_nested(stage, protected, {"path": "artifact"}, 0)
        member = stage.matched.get(locator)
        require(member is not None)
        path, sha256 = member
        parts = path.split("!")[1:]
        require(len(parts) == locator.count("/") + 1)
        return {"locator": locator, "path": [normalized(part) for part in parts],
                "type": "regular", "sha256": sha256}


def read_entry(layer, member, requested, workspace, budget):
    if not member.isfile() or member.issparse():
        return None
    path = archive_path(member.name, posix_tar=True)
    nested = {loc for loc in requested if loc is not None}
    with layer.extractfile(member) as stream:
        if not nested:
            budget.reserve(member.size)
            return path, stream_digest(stream, member.size), {}
        budget.reserve(member.size)
        require(member.size <= MAX_NESTED_MEMBER_BYTES)
        with tempfile.TemporaryDirectory(dir=workspace) as scratch:
            physical = Path(scratch) / "outer"
            require(copy_stream(stream, physical, member.size) == member.size)
            stage = Members(workspace, budget, nested)
            inspect_nested(stage, physical, {"path": path}, 0)
            return path, digest_file(physical), stage.matched


def layer_occurrences(archive, name, diff_id, requested, workspace, budget):
    physical = archive.layer(name, diff_id)
    result = {}
    try:
        with tarfile.open(physical, "r:") as layer:
            for index, member in enumerate(layer):
                budget.entry()
                if index in requested:
                    found = read_entry(layer, member, requested[index], workspace, budget)
                    if found is not None:
                        result[index] = found
            verify_trailer(layer, budget.max_bytes)
    finally:
        physical.unlink()
    return result


def requested_occurrences(report, component, evidence):
    by_layer = defaultdict(dict)
    for component_id, platform_id, layer, diff_id, entry, locator in evidence.entries:
        if component_id == component:
            selected = by_layer[platform_id, layer, diff_id]
            selected.setdefault(entry, set()).add(locator)
    coordinates = {}
    for row in report["inventory"]["layers"]:
        coordinate = (row["image"], row["index"])
        require(coordinate not in coordinates)
        coordinates[coordinate] = row["diff_id"]
    return by_layer, coordinates


def collect(path, workspace, report, component, evidence):
    wanted, coordinates = requested_occurrences(report, component, evidence)
    budget = Budget(IDENTITY_BUDGET, 1_000_000)
    found = {}
    with ImageArchive(path, workspace, budget) as archive:
        _, images = archive.images()
        expected_images = report["inventory"]["images"]
        require(len(images) == len(expected_images))
        expected_layer_count = 0
        for i, image in enumerate(images):
            config = image["config_digest"]
            manifest = image["manifest"]
            expected = expected_images[i]
            require(expected["index"] == i and expected["config_digest"] == config
                    and expected["manifest"] == manifest and expected["layers"] == len(image["layers"]))
            expected_layer_count += len(image["layers"])
            platform_id = platform(image)
            for j, (name, diff_id) in enumerate(zip(image["layers"], image["diff_ids"])):
                require(coordinates.get((i, j)) == diff_id)
                request = wanted.get((platform_id, j, diff_id))
                if request:
                    entries = layer_occurrences(archive, name, diff_id, request, workspace, budget)
                    for index, occurrence in entries.items():
                        found[i, j, index] = (platform_id, config, manifest, diff_id, occurrence)
        require(len(coordinates) == expected_layer_count)
    return found
