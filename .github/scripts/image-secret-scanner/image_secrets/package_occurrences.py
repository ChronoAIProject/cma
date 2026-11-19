"""Prove image occurrences using the same bounded layer and member readers as detection."""

import hashlib
import json
import tarfile
import tempfile
from collections import defaultdict
from pathlib import Path

from . import digest_frames, library_vectors, nested_certificates
from .archive import ImageArchive
from .common import MAX_NESTED_MEMBER_BYTES, Budget, archive_path, copy_stream, digest_bytes, digest_file
from .containers import identify, inspect_nested
from .gate import CERTIFICATE_ADVISORY
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

    def __init__(self, workspace, budget, requested, certificate_locator=None):
        self.workspace = workspace
        self.budget = budget
        self.requested = requested
        self.matched = {}
        self.certificate_locator = certificate_locator
        self.certificate = None
        self.names = set()
        self.duplicate = False

    def add(self, data, location):
        self.budget.reserve(len(data))
        if self.certificate_locator is not None and location["member"].isdigit():
            path = location["path"]
            self.duplicate |= path in self.names
            self.names.add(path)

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
                if locator == self.certificate_locator and location["path"] == nested_certificates.PATH:
                    self.certificate = nested_certificates.inspect_bundle(stored, lambda _der: None)
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


def read_entry(layer, member, requested, workspace, budget, reviewed_payload, databases):
    if not member.isfile() or member.issparse():
        return None
    path = archive_path(member.name, posix_tar=True)
    nested = {loc for loc in requested if loc is not None}
    with layer.extractfile(member) as stream:
        if not nested:
            budget.reserve(member.size)
            if reviewed_payload is not None and "digest_frames" in reviewed_payload[0]:
                return read_digest_candidate(stream, member, path, workspace, reviewed_payload[0])
            vectors = reviewed_payload if member.size <= library_vectors.MAX_LIBRARY_BYTES else None
            databases = databases if member.size <= library_vectors.MAX_DATABASE_BYTES else ()
            if vectors is None and not databases:
                return (path, stream_digest(stream, member.size), {}), {}, set()
            data = stream.read(member.size + 1)
            require(len(data) == member.size)
            checksum = digest_bytes(data)
            verified = {}
            if vectors is not None:
                expected, reviews = vectors
                if path == expected["path"] and checksum == expected["file_sha256"]:
                    metadata = json.dumps({"path": path, "link": member.linkname, "pax": member.pax_headers},
                                          ensure_ascii=False).encode()
                    rules = {rule for rule, _, _ in reviews}
                    verified = library_vectors.verify(data, metadata, expected["self_test"], rules)
            verified_databases = set()
            for expected in databases:
                db = expected["self_test"]["database"]
                if path == db["path"] and checksum == db["sha256"] and library_vectors.database_contains(
                        data, expected["path"], db["member_checksum"], debian=path.endswith(".md5sums")):
                    verified_databases.add((db["diff_id"], path, checksum, expected["path"], db["member_checksum"]))
            return (path, checksum, {}), verified, verified_databases
        budget.reserve(member.size)
        require(member.size <= MAX_NESTED_MEMBER_BYTES)
        with tempfile.TemporaryDirectory(dir=workspace) as scratch:
            physical = Path(scratch) / "outer"
            require(copy_stream(stream, physical, member.size) == member.size)
            certificate_locator = None
            checksum = digest_file(physical)
            if reviewed_payload is not None:
                expected, _ = reviewed_payload
                if ("nested_certificate" in expected and path == expected["path"]
                        and checksum == expected["file_sha256"] and identify(physical) == "zip"):
                    certificate_locator = expected["member"]["locator"]
            stage = Members(workspace, budget, nested, certificate_locator)
            inspect_nested(stage, physical, {"path": path}, 0)
            verified = ({CERTIFICATE_ADVISORY: stage.certificate}
                        if stage.certificate and not stage.duplicate else {})
            return (path, checksum, stage.matched), verified, set()


def read_digest_candidate(stream, member, path, workspace, expected):
    if member.size > digest_frames.MAX_BYTES or path != expected["path"]:
        return (path, stream_digest(stream, member.size), {}), {}, set()
    with tempfile.TemporaryDirectory(prefix="digest-candidate-", dir=workspace) as scratch:
        payload = Path(scratch) / "binary"
        require(copy_stream(stream, payload, member.size) == member.size)
        checksum = digest_file(payload)
        verified = {}
        if checksum == expected["file_sha256"]:
            metadata = json.dumps({"path": path, "link": member.linkname, "pax": member.pax_headers},
                                  ensure_ascii=False).encode()
            verified = digest_frames.verify(payload, metadata, expected["digest_frames"])
        return (path, checksum, {}), verified, set()


def layer_occurrences(archive, name, diff_id, requested, workspace, budget, reviewed_payloads, databases):
    physical = archive.layer(name, diff_id)
    result = {}
    try:
        with tarfile.open(physical, "r:") as layer:
            for index, member in enumerate(layer):
                budget.entry()
                if index in requested:
                    found = read_entry(layer, member, requested[index], workspace, budget, reviewed_payloads.get(index),
                                       databases.get(index, ()))
                    if found is not None:
                        result[index] = found
            verify_trailer(layer, budget.max_bytes)
    finally:
        physical.unlink()
    return result


def requested_occurrences(report, component, evidence):
    by_layer = defaultdict(dict)
    reviewed_payloads = defaultdict(dict)
    databases = defaultdict(lambda: defaultdict(list))
    for component_id, platform_id, layer, diff_id, entry, locator in evidence.entries:
        if component_id == component:
            selected = by_layer[platform_id, layer, diff_id]
            selected.setdefault(entry, set()).add(locator)
            record = evidence.entries[component_id, platform_id, layer, diff_id, entry, locator]
            if record is not None and any(proof in record[0] for proof in
                                          ("self_test", "digest_frames", "nested_certificate")):
                reviewed_payloads[platform_id, layer, diff_id][entry] = record
            if record is not None and "self_test" in record[0]:
                db = record[0]["self_test"]["database"]
                db_layer = (platform_id, db["layer"], db["diff_id"])
                by_layer[db_layer].setdefault(db["entry"], set()).add(None)
                databases[db_layer][db["entry"]].append(record[0])
    coordinates = {}
    for row in report["inventory"]["layers"]:
        coordinate = (row["image"], row["index"])
        require(coordinate not in coordinates)
        coordinates[coordinate] = row["diff_id"]
    return by_layer, coordinates, reviewed_payloads, databases


def collect(path, workspace, report, component, evidence, *, vector_checks=None, database_checks=None):
    wanted, coordinates, reviewed_payloads, databases = requested_occurrences(report, component, evidence)
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
                layer = (platform_id, j, diff_id)
                request = wanted.get(layer)
                if request:
                    entries = layer_occurrences(archive, name, diff_id, request, workspace, budget,
                                                reviewed_payloads.get(layer, {}), databases.get(layer, {}))
                    for index, (occurrence, verified, verified_databases) in entries.items():
                        coordinate = (i, j, index)
                        found[coordinate] = (platform_id, config, manifest, diff_id, occurrence)
                        if vector_checks is not None and verified:
                            vector_checks[coordinate] = verified
                        if database_checks is not None and verified_databases:
                            database_checks[coordinate] = verified_databases
        require(len(coordinates) == expected_layer_count)
    return found
