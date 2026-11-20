"""Prove a path-field detection on an installed, authenticated Debian trigger file."""

import re

from .common import digest_bytes, tar_metadata
from .gate import DEBIAN_CONTROL_ADVISORY, DEBIAN_CONTROL_RULE, valid_digest
from .package_evidence import MATCH_FIELDS, fields, image_binding, matches, require

NAME = re.compile(r"[a-z0-9][a-z0-9+.-]{1,199}")
ARCHITECTURE = re.compile(r"[a-z0-9][a-z0-9-]{0,31}")
CONTROL_ARCHIVES = frozenset({"control.tar.gz", "control.tar.xz", "control.tar.zst"})
PATH_PREFIX = b'{"path": "'


def validate(row, package, *, portable=False):
    proof = row["debian_control"]
    image_binding(proof, {"view_sha256", "span"}, portable=portable, finding_fields={"finding_id"})
    require(row["component"] == "runtime" and row["member"] is None and package["authority_type"] == "debian-index"
            and matches(NAME, package["name"]) and matches(ARCHITECTURE, package["architecture"])
            and valid_digest(proof["view_sha256"]) and (portable or valid_digest(proof["finding_id"])))
    name, architecture = package["name"], package["architecture"]
    require(row["path"] in {f"var/lib/dpkg/info/{name}.triggers", f"var/lib/dpkg/info/{name}:{architecture}.triggers"})
    package_member = row["package_member"]
    path = package_member["path"]
    require(package_member["locator"].startswith("1/0/") and len(path) == 3
            and path[0] in CONTROL_ARCHIVES and path[1:] == ["decoded", "triggers"])
    span = proof["span"]
    fields(span, {"offset", "size", "sha256"})
    require(type(span["offset"]) is int and span["offset"] >= len(PATH_PREFIX)
            and type(span["size"]) is int and span["size"] > 0
            and span["offset"] + span["size"] <= len(PATH_PREFIX) + len(row["path"])
            and valid_digest(span["sha256"]))
    require(len(row["matches"]) == 1)
    [match] = row["matches"]
    fields(match, MATCH_FIELDS)
    require(match["rule"] == DEBIAN_CONTROL_RULE and match["explanation"] == DEBIAN_CONTROL_ADVISORY
            and match["candidate_sha256"] == span["sha256"] and type(match["line"]) is int and match["line"] == 1
            and valid_digest(match["proof_sha256"]))
    return {(DEBIAN_CONTROL_RULE, span["sha256"], 1): match}


def verify(path, member, proof):
    if member.linkname:
        return False
    view = tar_metadata(path, member)
    if digest_bytes(view) != proof["view_sha256"] or not view.startswith(PATH_PREFIX + path.encode("ascii") + b'"'):
        return False
    start = proof["span"]["offset"]
    candidate = view[start:start + proof["span"]["size"]]
    # A duplicate in link or PAX data must never be attributed to the public path.
    return (digest_bytes(candidate) == proof["span"]["sha256"] and view.find(candidate) == start
            and view.find(candidate, start + 1) == -1)
