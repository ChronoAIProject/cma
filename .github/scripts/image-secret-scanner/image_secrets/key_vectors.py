"""Prove the one package-bound upstream self-test vector set; keep raw findings."""

import hashlib
import json
import re
import tarfile
from pathlib import Path

from .common import ScanError, archive_path, json_bytes
from .vector_paths import VectorPaths

REGISTRY = Path(__file__).resolve().parents[1] / "upstream-test-vectors.json"
STATUS = "var/lib/dpkg/status"
LIST = "var/lib/dpkg/info/libgnutls30t64:amd64.list"
PROVENANCE = "usr/share/cma-runtime/package-provenance/gnutls.json"
HEADER = re.compile(rb"-----BEGIN (?:[A-Z0-9 ]+ )?PRIVATE KEY-----")
PEM = re.compile(rb"-----BEGIN ([A-Z0-9 ]*PRIVATE KEY)-----.{0,32768}?-----END \1-----", re.S)
LIMIT = 8 * 1024 * 1024


def sha(data):
    return hashlib.sha256(data).hexdigest()


def policy():
    value = json_bytes(REGISTRY.read_bytes())
    if (
        not isinstance(value, dict)
        or set(value) != {"schema", "upstream", "package", "library", "blocks"}
        or value["schema"] != "cma-upstream-self-tests-v1"
    ):
        raise ScanError("invalid-upstream-vector-policy")
    # No arbitrary caller-controlled path, package or source family can become
    # an exception. Additional packages require a separate source review.
    if (
        value["package"].get("name") != "libgnutls30t64"
        or value["package"].get("version") != "3.8.9-3+deb13u4"
        or value["package"].get("architecture") != "amd64"
        or value["library"].get("path") != "usr/lib/x86_64-linux-gnu/libgnutls.so.30.40.3"
        or value["upstream"].get("version") != "3.8.9"
        or value["upstream"].get("member") != "gnutls-3.8.9/lib/crypto-selftests-pk.c"
    ):
        raise ScanError("invalid-upstream-vector-identity")
    hashes = [
        value["package"].get("sha256"),
        value["library"].get("sha256"),
        value["upstream"].get("archive_sha256"),
        value["upstream"].get("file_sha256"),
    ]
    blocks = value["blocks"]
    if (
        not isinstance(blocks, list)
        or len(blocks) != 12
        or any(
            not isinstance(digest, str) or re.fullmatch(r"[a-f0-9]{64}", digest) is None for digest in hashes
        )
    ):
        raise ScanError("invalid-upstream-vector-digest")
    offsets = set()
    for block in blocks:
        if (
            not isinstance(block, dict)
            or set(block) != {"offset", "length", "sha256"}
            or type(block["offset"]) is not int
            or block["offset"] < 0
            or block["offset"] in offsets
            or type(block["length"]) is not int
            or not 1 <= block["length"] <= 32768
            or not isinstance(block["sha256"], str)
            or re.fullmatch(r"[a-f0-9]{64}", block["sha256"]) is None
        ):
            raise ScanError("invalid-upstream-vector-occurrence")
        offsets.add(block["offset"])
    return value


def package_identity_matches(status, package):
    try:
        paragraphs = status.decode("utf-8").split("\n\n")
    except UnicodeError:
        return False
    packages = []
    for paragraph in paragraphs:
        fields = {}
        for line in paragraph.splitlines():
            if not line or line[0].isspace():
                continue
            key, separator, value = line.partition(":")
            # Debian control files permit empty fields, such as Conffiles:,
            # and optional horizontal whitespace after the field separator.
            if not separator or re.fullmatch(r"[A-Za-z0-9][A-Za-z0-9-]*", key) is None or key in fields:
                return False
            fields[key] = value.lstrip(" \t")
        if fields.get("Package") == package["name"]:
            packages.append(fields)
    return (
        len(packages) == 1
        and packages[0].get("Version") == package["version"]
        and packages[0].get("Architecture") == package["architecture"]
        and packages[0].get("Status") == "install ok installed"
    )


def package_matches(status, listing, expected):
    try:
        names = listing.decode("utf-8").splitlines()
    except UnicodeError:
        return False
    return (
        package_identity_matches(status, expected["package"])
        and names.count("/" + expected["library"]["path"]) == 1
    )


def provenance_bytes(expected):
    value = {
        "schema": "runtime-package-provenance-v1",
        "package": expected["package"],
        "library": expected["library"],
    }
    return (json.dumps(value, sort_keys=True, separators=(",", ":")) + "\n").encode()


def prove(data, status, listing, provenance, expected):
    if (
        sha(data) != expected["library"]["sha256"]
        or not package_matches(status, listing, expected)
        or provenance != provenance_bytes(expected)
    ):
        return None
    return vector_proof(
        data,
        expected["library"],
        expected["blocks"],
        expected["package"]["sha256"],
        sha(REGISTRY.read_bytes()),
        sha(provenance),
    )


def vector_proof(data, library, blocks, package_digest, policy_digest, provenance_digest):
    matches = list(PEM.finditer(data))
    occurrences = [
        {"offset": block.start(), "length": len(block[0]), "sha256": sha(block[0])} for block in matches
    ]
    if sha(data) != library["sha256"] or len(HEADER.findall(data)) != len(blocks) or occurrences != blocks:
        return None
    return {
        "file_sha256": sha(data),
        "occurrences": occurrences,
        "package_sha256": package_digest,
        "policy_sha256": policy_digest,
        "provenance_sha256": provenance_digest,
    }


def registry_for(component):
    if component == "runtime-omarchy":
        return REGISTRY.with_name("omarchy-upstream-test-vectors.json")
    return REGISTRY


def reviewed_vectors(result, component):
    if "upstream_test_vectors_sha256" not in result["policy"]:
        if "upstream_test_vectors" in result:
            raise ScanError("unbound-upstream-vector-proof")
        return set()
    if component == "runtime-omarchy":
        from .arch_vectors import reviewed

        return reviewed(result)
    expected = policy()
    digest = sha(REGISTRY.read_bytes())
    entries = [
        {
            "library": expected["library"],
            "blocks": expected["blocks"],
            "package_sha256": expected["package"]["sha256"],
            "provenance_sha256": sha(provenance_bytes(expected)),
        }
    ]
    if component not in ("runtime", "runtime-desktop"):
        raise ScanError("unbound-upstream-vector-proof")
    return verify_proofs(result, digest, entries)


def verify_proofs(result, digest, entries):
    proofs = result.get("upstream_test_vectors")
    if result["policy"].get("upstream_test_vectors_sha256") != digest or not isinstance(proofs, list):
        raise ScanError("unbound-upstream-vector-proof")
    expected_files = {entry["library"]["sha256"]: entry for entry in entries}
    candidates = {item["finding_id"]: item for item in result["findings"]}
    verified = set()
    for proof in proofs:
        if not isinstance(proof, dict) or set(proof) != {
            "finding_id",
            "file_sha256",
            "occurrences",
            "package_sha256",
            "policy_sha256",
            "provenance_sha256",
        }:
            raise ScanError("invalid-upstream-vector-proof")
        expected = expected_files.get(proof["file_sha256"])
        if expected is None:
            raise ScanError("invalid-upstream-vector-proof")
        finding = candidates.get(proof["finding_id"], {})
        if (
            proof["finding_id"] in verified
            or proof["policy_sha256"] != digest
            or proof["file_sha256"] != expected["library"]["sha256"]
            or proof["package_sha256"] != expected["package_sha256"]
            or proof["provenance_sha256"] != expected["provenance_sha256"]
            or proof["occurrences"] != expected["blocks"]
            or finding.get("scope") != "layer"
            or "member" in finding
            or finding.get("path") != "path-sha256:" + sha(expected["library"]["path"].encode())
            or finding.get("rule") not in ("content.private-key-header", "gitleaks.private-key")
            or any(
                type(finding.get(key)) is not int or finding[key] < 0 for key in ("image", "layer", "entry")
            )
            or finding.get("file_sha256", expected["library"]["sha256"]) != expected["library"]["sha256"]
        ):
            raise ScanError("invalid-upstream-vector-proof")
        verified.add(proof["finding_id"])
    return verified


class VectorState:
    def __init__(self):
        self.expected = policy()
        self.paths = VectorPaths((self.expected["library"]["path"], STATUS, LIST, PROVENANCE))
        self.status = b""
        self.listing = b""
        self.provenance = b""

    def layer(self, path, image, layer, findings):
        library = self.expected["library"]["path"]
        files, candidates, seen, ambiguous = {}, [], set(), set()
        with tarfile.open(path, "r:") as archive:
            for index, entry in enumerate(archive):
                name = archive_path(entry.name)
                self.paths.observe(name, entry)
                # Conservative package-state invalidation on any whiteout.
                if any(part.startswith(".wh.") for part in name.split("/")):
                    self.status = self.listing = self.provenance = b""
                if name not in (library, STATUS, LIST, PROVENANCE):
                    continue
                if name in seen:
                    ambiguous.add(name)
                seen.add(name)
                files[name] = b""
                if entry.isfile() and entry.size <= LIMIT:
                    with archive.extractfile(entry) as stream:
                        files[name] = stream.read(LIMIT + 1)
                if name == library:
                    candidates.append((index, files[name]))
        if ambiguous or self.paths.redirected:
            self.status = self.listing = self.provenance = b""
            return
        self.status = files.get(STATUS, self.status)
        self.listing = files.get(LIST, self.listing)
        self.provenance = files.get(PROVENANCE, self.provenance)
        for index, data in candidates:
            proof = prove(data, self.status, self.listing, self.provenance, self.expected)
            if proof is not None:
                findings.upstream_vectors[(image, layer, index, library)] = proof
