"""A private reviewer-GO receipt authorizes only the exact reviewed source."""

import hashlib
from pathlib import Path

from identity import approved, canonical_hash
from safety import Refusal, read_private


def source_fingerprint():
    root = Path(__file__).parent
    records = []
    for path in sorted([*root.glob("*.py"), root / "approved.json"]):
        if path.is_symlink():
            raise Refusal("reviewed_source_symlink")
        records.append([path.name, hashlib.sha256(path.read_bytes()).hexdigest()])
    return canonical_hash(records)


def require_go():
    receipt = read_private("review-go.json", {})
    if (receipt.get("verdict") != "GO" or receipt.get("source_sha256") != source_fingerprint()
            or receipt.get("manifest_sha256") != canonical_hash(approved())):
        raise Refusal("exact_source_security_review_go_required")
