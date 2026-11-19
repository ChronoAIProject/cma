"""Prove image occurrences using the same bounded layer and member readers as detection."""

import hashlib
import mmap
import tarfile
import tempfile
from collections import defaultdict
from pathlib import Path

from . import (
    arch_trust,
    debian_control,
    derived_payload,
    digest_frames,
    distribution_config,
    library_vectors,
    nested_certificates,
    reviewed_literals,
    wheel_record,
)
from .archive import ImageArchive
from .common import (
    MAX_NESTED_MEMBER_BYTES,
    Budget,
    archive_path,
    copy_stream,
    digest_bytes,
    digest_file,
    tar_metadata,
)
from .containers import identify, inspect_nested
from .gate import (
    ARCH_CERTIFICATE_ADVISORY,
    CERTIFICATE_ADVISORY,
    DEBIAN_CONTROL_ADVISORY,
    DISTRIBUTION_ADVISORY,
)
from .package_evidence import NESTED_LOCATOR, PLATFORM, matches, normalized, require
from .policy import Findings, inspect_tar_xattrs
from .tar_trailer import verify_trailer
from .trust_store import MAX_CERT_FILE, TrustStore

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

    def __init__(self, workspace, budget, requested, certificate_locator=None, literal_proofs=None):
        self.workspace = workspace
        self.budget = budget
        self.requested = requested
        self.matched = {}
        self.certificate_locator = certificate_locator
        self.certificate = None
        self.names = set()
        self.duplicate = False
        self.rejected_xattrs = set()
        self.literal_proofs = literal_proofs or {}
        self.literal_metadata = defaultdict(set)
        self.checked_literals = {}

    def add(self, data, location):
        self.budget.reserve(len(data))
        locator = location.get("member")
        if locator is None:
            return
        if (self.certificate_locator is not None or self.literal_proofs) and locator.isdigit():
            path = location["path"]
            self.duplicate |= path in self.names
            self.names.add(path)
        if locator in self.literal_proofs:
            proof = self.literal_proofs[locator]
            self.literal_metadata[locator].update(
                entry["rule"] for entry in proof["findings"]
                if next(reviewed_literals.occurrences(data, entry["rule"]), None) is not None)

    def add_tar_metadata(self, name, member, location, *, ascii_only=False):
        self.add(tar_metadata(name, member, ascii_only=ascii_only), location)
        if not clean_xattrs(member, self.budget, self.workspace):
            self.rejected_xattrs.add(location["member"])

    def inspect_path(self, _location, _path):
        # A payload finding cannot be proved from path or link metadata.
        return None

    def file(self, stream, size, location, _original_path, depth, *, regular=True):
        self.budget.reserve(size)
        require(size <= MAX_NESTED_MEMBER_BYTES)
        locator = location["member"]
        has_child = any(name.startswith(locator + "/") for name in self.requested)
        if not regular or locator in self.rejected_xattrs or (locator not in self.requested and not has_child):
            # ar streams members over one pipe; each extent must be consumed
            # even when the requested proof belongs to a later member.
            stream_digest(stream, size)
            return
        with tempfile.TemporaryDirectory(dir=self.workspace) as child:
            stored = Path(child) / "member"
            require(copy_stream(stream, stored, size) == size)
            if locator in self.requested:
                self.matched[locator] = (location["path"], digest_file(stored))
                if locator in self.literal_proofs:
                    self.checked_literals[locator] = reviewed_literals.verify(stored, b"", self.literal_proofs[locator])
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


def clean_xattrs(member, budget, workspace):
    findings = Findings(None)
    inspect_tar_xattrs(member, {"scope": "layer", "path": ""}, findings, budget, workspace)
    return not findings.items


def read_entry(layer, member, requested, workspace, budget, reviewed_payloads, databases):
    if not member.isfile() or member.issparse() or not clean_xattrs(member, budget, workspace):
        return None
    path = archive_path(member.name, posix_tar=True)
    nested = {loc for loc in requested if loc is not None}
    reviewed_payload = reviewed_payloads.get(None)
    with layer.extractfile(member) as stream:
        if not nested:
            budget.reserve(member.size)
            if reviewed_payload is not None:
                expected, _ = reviewed_payload
                if "debian_control" in expected:
                    return read_control_candidate(stream, member, path, expected)
                if ({"distribution_config", "digest_frames", "reviewed_literals", "derived_payload"}.intersection(expected)
                        or "executable" in expected.get("self_test", {})):
                    return read_payload_candidate(stream, member, path, workspace, *reviewed_payload)
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
                    metadata = tar_metadata(path, member)
                    rules = {rule for rule, _, _ in reviews}
                    verified = library_vectors.verify(data, metadata, expected["self_test"], rules)
            verified_databases = set()
            for member_path, db in databases:
                if path == db["path"] and checksum == db["sha256"] and library_vectors.database_contains(
                        data, member_path, db["member_checksum"], debian=path.endswith(".md5sums")):
                    verified_databases.add((db["diff_id"], path, checksum, member_path, db["member_checksum"]))
            return (path, checksum, {}), verified, verified_databases
        budget.reserve(member.size)
        require(member.size <= MAX_NESTED_MEMBER_BYTES)
        with tempfile.TemporaryDirectory(dir=workspace) as scratch:
            physical = Path(scratch) / "outer"
            require(copy_stream(stream, physical, member.size) == member.size)
            certificate_locator = None
            checksum = digest_file(physical)
            proofs = {locator: expected for locator, (expected, _) in reviewed_payloads.items()
                      if locator is not None and path == expected["path"] and checksum == expected["file_sha256"]}
            container = identify(physical) if proofs else None
            record_sha256 = (wheel_record.verify(physical, budget) if container == "zip" and any(
                "derived_payload" in value for value in proofs.values()) else None)
            literal_proofs = {}
            derived = set()
            for locator, expected in proofs.items():
                origin = expected.get("derived_payload")
                if origin is not None:
                    if record_sha256 != origin["record_sha256"]:
                        continue
                    derived.add(locator)
                if "nested_certificate" in expected and container == "zip":
                    certificate_locator = locator
                if "reviewed_literals" in expected:
                    literal_proofs[locator] = expected["reviewed_literals"]
            stage = Members(workspace, budget, nested, certificate_locator, literal_proofs)
            inspect_nested(stage, physical, {"path": path}, 0)
            verified = ({CERTIFICATE_ADVISORY: stage.certificate}
                        if stage.certificate and not stage.duplicate else {})
            if literal_proofs and not stage.duplicate:
                verified["nested_literals"] = {
                    locator: {rule: claim for rule, claim in checks.items() if rule not in stage.literal_metadata[locator]}
                    for locator, checks in stage.checked_literals.items()}
            if derived:
                verified["nested_derived"] = derived
            return (path, checksum, stage.matched), verified, set()


def read_control_candidate(stream, member, path, expected):
    checksum = stream_digest(stream, member.size)
    verified = {}
    if path == expected["path"] and checksum == expected["file_sha256"]:
        proof = expected["debian_control"]
        if debian_control.verify(path, member, proof):
            verified[DEBIAN_CONTROL_ADVISORY] = proof
    return (path, checksum, {}), verified, set()


def read_payload_candidate(stream, member, path, workspace, expected, reviews):
    if path != expected["path"] or not 0 < member.size <= reviewed_literals.MAX_BYTES:
        return (path, stream_digest(stream, member.size), {}), {}, set()
    # Large browser executables must not consume their full size in Python heap.
    with tempfile.TemporaryDirectory(prefix="reviewed-payload-", dir=workspace) as scratch:
        payload = Path(scratch) / "member"
        require(copy_stream(stream, payload, member.size) == member.size)
        checksum = digest_file(payload)
        verified = {}
        if checksum == expected["file_sha256"]:
            metadata = tar_metadata(path, member)
            if "digest_frames" in expected:
                verified.update(digest_frames.verify(payload, metadata, expected["digest_frames"]))
            origin = expected.get("derived_payload")
            is_mime = origin is not None and origin["kind"] == "mime-index"
            if "reviewed_literals" in expected:
                verified["reviewed_literals"] = reviewed_literals.verify(
                    payload, metadata, expected["reviewed_literals"], mime_index=path if is_mime else None)
            if is_mime and verified["reviewed_literals"]:
                verified["derived_payload"] = True
            vector = expected.get("self_test")
            eligible = vector is not None and (member.mode & 0o111 if "executable" in vector
                                               else member.size <= library_vectors.MAX_LIBRARY_BYTES)
            distribution = expected.get("distribution_config")
            compiled = origin is not None and origin["kind"] == "compiled-executable"
            if eligible or member.mode & 0o111 and (distribution is not None or compiled):
                with payload.open("rb") as source, mmap.mmap(source.fileno(), 0, access=mmap.ACCESS_READ) as data:
                    if eligible:
                        verified.update(library_vectors.verify(data, metadata, vector, {rule for rule, _, _ in reviews}))
                    if (distribution is not None and member.mode & 0o111 and library_vectors.elf_subject(data, executable=True)
                            and distribution_config.verify(data, distribution)):
                        verified[DISTRIBUTION_ADVISORY] = True
                    if compiled and derived_payload.verify_compiled(data, origin):
                        verified["derived_payload"] = True
        return (path, checksum, {}), verified, set()


def read_arch_trust_entry(layer, entry, expected, workspace, budget):
    path = archive_path(entry.name, posix_tar=True)
    if path != expected["path"] or entry.issparse() or not clean_xattrs(entry, budget, workspace):
        return None
    if path == arch_trust.ALIAS:
        if entry.issym() and entry.linkname == expected["link_target"]:
            return (path, None, {}), {ARCH_CERTIFICATE_ADVISORY: {"link_target": entry.linkname}}, set()
        return None
    if not entry.isfile() or not 0 < entry.size <= MAX_CERT_FILE:
        return None
    budget.reserve(entry.size)
    with tempfile.TemporaryDirectory(dir=workspace) as scratch:
        physical = Path(scratch) / "certificate"
        with layer.extractfile(entry) as source:
            require(copy_stream(source, physical, entry.size) == entry.size)
        if digest_file(physical) != expected["file_sha256"]:
            return None
        checked = arch_trust.inspect_file(physical, path, lambda _der: None)
        if checked is not None:
            return ((path, checked["member_sha256"], {}), {ARCH_CERTIFICATE_ADVISORY: checked}, set())
    return None


def layer_occurrences(archive, name, diff_id, requested, workspace, budget, reviewed_payloads, databases,
                      arch_entries, *, namespace=None, layer_index=None):
    physical = archive.layer(name, diff_id)
    result = {}
    try:
        with tarfile.open(physical, "r:") as layer:
            for index, member in enumerate(layer):
                budget.entry()
                if namespace is not None:
                    # The namespace is derived from the digest-verified archive. No
                    # detector observation can supply a missing link or override a whiteout.
                    location = {"path": archive_path(member.name, posix_tar=True), "layer": layer_index}
                    namespace.observe(member, location, None, None)
                if index in requested:
                    if index in arch_entries:
                        found = read_arch_trust_entry(layer, member, arch_entries[index], workspace, budget)
                    else:
                        found = read_entry(layer, member, requested[index], workspace, budget,
                                           reviewed_payloads.get(index, {}), databases.get(index, ()))
                    if found is not None:
                        result[index] = found
            verify_trailer(layer, budget.max_bytes)
    finally:
        physical.unlink()
    return result


def requested_occurrences(report, component, evidence):
    by_layer = defaultdict(dict)
    reviewed_payloads = defaultdict(lambda: defaultdict(dict))
    databases = defaultdict(lambda: defaultdict(list))
    arch_entries = defaultdict(dict)
    for component_id, platform_id, layer, diff_id, entry, locator in evidence.entries:
        if component_id == component:
            selected = by_layer[platform_id, layer, diff_id]
            selected.setdefault(entry, set()).add(locator)
            record = evidence.entries[component_id, platform_id, layer, diff_id, entry, locator]
            row, _ = record
            if any(key in row for key in ("self_test", "digest_frames", "nested_certificate",
                                          "distribution_config", "debian_control", "reviewed_literals", "derived_payload")):
                reviewed_payloads[platform_id, layer, diff_id][entry][locator] = record
            for key in ("self_test", "distribution_config"):
                if key in row:
                    db = row[key]["database"]
                    db_layer = (platform_id, db["layer"], db["diff_id"])
                    by_layer[db_layer].setdefault(db["entry"], set()).add(None)
                    databases[db_layer][db["entry"]].append((row["path"], db))
    if component == arch_trust.COMPONENT and evidence.arch_trust is not None:
        packet = evidence.arch_trust
        arch_layer = (arch_trust.PLATFORM, arch_trust.LAYER, packet["base"]["layer_diff_id"])
        arch_rows = list(packet["files"].values())
        if packet["alias"] is not None:
            arch_rows.append(packet["alias"])
        for row in arch_rows:
            entry = row["entry"]
            require(entry not in by_layer[arch_layer])
            by_layer[arch_layer][entry] = {None}
            arch_entries[arch_layer][entry] = row
    coordinates = {}
    for row in report["inventory"]["layers"]:
        coordinate = (row["image"], row["index"])
        require(coordinate not in coordinates)
        coordinates[coordinate] = row["diff_id"]
    return by_layer, coordinates, reviewed_payloads, databases, arch_entries


def collect(path, workspace, report, component, evidence, *, vector_checks=None, database_checks=None):
    wanted, coordinates, reviewed_payloads, databases, arch_entries = requested_occurrences(report, component, evidence)
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
            namespace = None
            alias = None
            if component == arch_trust.COMPONENT and platform_id == arch_trust.PLATFORM and evidence.arch_trust:
                packet = evidence.arch_trust
                if image["diff_ids"] and image["diff_ids"][arch_trust.LAYER] == packet["base"]["layer_diff_id"]:
                    alias = packet["alias"]
                    if alias is not None:
                        namespace = TrustStore()
            for j, (name, diff_id) in enumerate(zip(image["layers"], image["diff_ids"])):
                require(coordinates.get((i, j)) == diff_id)
                layer = (platform_id, j, diff_id)
                request = wanted.get(layer, {})
                if request or namespace is not None:
                    entries = layer_occurrences(archive, name, diff_id, request, workspace, budget,
                                                reviewed_payloads.get(layer, {}), databases.get(layer, {}),
                                                arch_entries.get(layer, {}), namespace=namespace, layer_index=j)
                    for index, (occurrence, verified, verified_databases) in entries.items():
                        coordinate = (i, j, index)
                        found[coordinate] = (platform_id, config, manifest, diff_id, occurrence)
                        if vector_checks is not None and verified:
                            vector_checks[coordinate] = verified
                        if database_checks is not None and verified_databases:
                            database_checks[coordinate] = verified_databases
            if (namespace is not None and vector_checks is not None
                    and namespace.direct_alias(arch_trust.ALIAS, arch_trust.TLS, arch_trust.LAYER)):
                check = vector_checks.get((i, arch_trust.LAYER, alias["entry"]), {}).get(ARCH_CERTIFICATE_ADVISORY)
                if check is not None:
                    check["namespace_verified"] = True
        require(len(coordinates) == expected_layer_count)
    return found
