"""Review-bound JavaScript contexts in explicitly scoped generated assets."""
import hashlib
import json
import os
import re
import subprocess
from pathlib import Path

from image_secrets.common import ScanError
from image_secrets.gate import valid_digest

PREFIXES = ("usr/share/nginx/html/assets/", "usr/share/nginx/current/assets/")
PREFIX = PREFIXES[0]
RULE = "gitleaks.image-secret-assignment"
CONTEXT = "comparison-fragment-v1"
CHUNK = "RichDocumentEditor"
OUTPUTS = {prefix: re.compile(re.escape(prefix + CHUNK) + r"-[A-Za-z0-9_-]{8}\.js")
           for prefix in PREFIXES}
MAX_BYTES = 16 * 1024 * 1024
RUNNER = Path(__file__).with_name("image_bundle_syntax.cjs")


def scope(path):
    for prefix, pattern in OUTPUTS.items():
        if pattern.fullmatch(path):
            return hashlib.sha256((prefix + CHUNK).encode()).hexdigest()
    return None


def entries(catalog):
    rows = catalog.get("bundle_candidates", [])
    if not isinstance(rows, list) or len(rows) > 8:
        raise ScanError("invalid-bundle-contexts")
    observed = set()
    fields = {"component", "asset_prefix", "chunk", "rule", "candidate_sha256", "syntax", "classification", "reason"}
    for row in rows:
        if (not isinstance(row, dict) or set(row) != fields or row["component"] != "web"
                or row["asset_prefix"] not in PREFIXES or row["chunk"] != CHUNK or row["rule"] != RULE
                or row["syntax"] != CONTEXT or not valid_digest(row["candidate_sha256"])
                or row["classification"] != "public-material" or not isinstance(row["reason"], str)
                or not row["reason"].strip()):
            raise ScanError("invalid-bundle-context")
        identity = hashlib.sha256((row["asset_prefix"] + row["chunk"]).encode()).hexdigest()
        key = ("bundle", "web", identity, RULE, row["candidate_sha256"])
        if key in observed:
            raise ScanError("duplicate-bundle-context")
        observed.add(key)
        for exact in catalog["entries"]:
            if (exact["classification"] == "REAL" and exact["component"] == "web" and exact["rule"] == RULE
                    and exact.get("candidate_sha256") == row["candidate_sha256"]
                    and scope(exact["path"]) == identity):
                raise ScanError("bundle-context-conflicts-with-real")
        yield key, row


def prove(payload, findings):
    if not payload or len(payload) > MAX_BYTES:
        return set()
    try:
        request = {"text": payload.decode("utf-8"), "findings": [
            {key: finding.get(key) for key in ("finding_id", "candidate_sha256", "line")}
            for finding in findings if finding["rule"] == RULE]}
        env = {key: value for key, value in os.environ.items()
               if key in {"PATH", "HOME", "LANG", "LC_ALL", "TMPDIR"}}
        result = subprocess.run(["node", "--max-old-space-size=512", str(RUNNER)],
                                input=json.dumps(request, ensure_ascii=False).encode(), env=env,
                                capture_output=True, timeout=30, check=False)
        if result.returncode:
            return set()
        proven = json.loads(result.stdout)
        if (not isinstance(proven, list) or not all(isinstance(item, str) for item in proven)
                or not set(proven) <= {finding["finding_id"] for finding in findings}):
            return set()
        return set(proven)
    except (OSError, UnicodeError, ValueError, subprocess.TimeoutExpired):
        # Syntax or parser failures leave new content unresolved, never approved.
        return set()
