"""Bind one staging detector finding to the reviewed GnuTLS self-test bytes."""

import hashlib
from dataclasses import dataclass, field

from image_secrets.policy import PRIVATE

RULE = "content.private-key-header"
PATH = "usr/lib/x86_64-linux-gnu/libgnutls.so.30.40.3"
FILE_SHA256 = "4b14edde42e7cbeefd8a6d88d035e096f2e6412a535388bb1d073a1c196e2822"
MAX_BYTES = 4 * 1024 * 1024
COUNT = 12
STAGING = "ChronoAIProject/cma/.github/workflows/staging-cd.yml@refs/heads/develop"
REASON = "All twelve raw headers begin reviewed GnuTLS power-on self-test vectors"
RAW_FIELDS = {"rule", "scope", "image", "layer", "entry", "path", "finding_id"}


@dataclass
class HeaderProof:
    vectors: tuple
    confirmed: set = field(default_factory=set)

    def needs(self, path, size, findings):
        return path == PATH and 0 < size <= MAX_BYTES and any(f["rule"] == RULE for f in findings)

    def observe(self, payload, file_digest, findings):
        if file_digest != FILE_SHA256:
            return
        # Use the frozen detector's own pattern. A new header anywhere,
        # including between two genuine vectors, invalidates the whole proof.
        offsets = tuple(match.start() for match in PRIVATE.finditer(payload))
        if offsets != tuple(offset for offset, _, _ in self.vectors):
            return
        view = memoryview(payload)
        for offset, size, digest in self.vectors:
            if offset + size > len(view) or hashlib.sha256(view[offset:offset + size]).hexdigest() != digest:
                return
        self.confirmed.update(f["finding_id"] for f in findings if f["rule"] == RULE and set(f) == RAW_FIELDS)


def staging_proof(component, env, entries):
    if (component != "runtime" or env.get("CD_COMPONENT") != component
            or env.get("GITHUB_EVENT_NAME") != "push" or env.get("GITHUB_REF") != "refs/heads/develop"
            or env.get("GITHUB_WORKFLOW_REF") != STAGING):
        return None
    # catalog_index has already checked each public vector's provenance.
    # Include competing REAL entries in this selection so they prevent approval.
    rows = [r for r in entries if r["component"] == component and r["scope"] == "layer" and r["path"] == PATH
            and r["file_sha256"] == FILE_SHA256 and r["rule"] == "gitleaks.private-key"]
    if len(rows) != COUNT or any(r["classification"] != "public-material" for r in rows):
        return None
    vectors = [(r.get("header_offset"), r.get("vector_bytes"), r["candidate_sha256"]) for r in rows]
    if any(type(offset) is not int or type(size) is not int or offset < 0 or size <= 0
           or offset + size > MAX_BYTES for offset, size, _ in vectors):
        return None
    vectors.sort()
    if (len({digest for _, _, digest in vectors}) != COUNT
            or any(left[0] + left[1] > right[0] for left, right in zip(vectors, vectors[1:]))):
        return None
    return HeaderProof(tuple(vectors))
