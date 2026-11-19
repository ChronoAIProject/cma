"""Complete layer detector evidence; the caller authenticates its producer.

No policy decision is reusable. Fresh traversal supplies the verified diff-ID,
every staged input and current locations before detector work may be skipped.
"""
import hashlib
import json
import platform
import re
import shutil
import sys
from pathlib import Path

from .common import ScanError, digest_file, json_bytes

SCHEMA = "image-layer-detector-cache-v1"
MAX_BYTES = 64 * 1024 * 1024
MAX_LAYERS = 1024
MAX_INPUTS = 100000
MAX_ROWS = 100000
HEX = re.compile(r"[a-f0-9]{64}")
DIFF_ID = re.compile(r"sha256:[a-f0-9]{64}")


def canonical(value):
    return json.dumps(value, sort_keys=True, separators=(",", ":"), allow_nan=False).encode()


def require(value):
    if not value:
        raise ScanError("layer-cache-evidence-invalid")


def fingerprint(scanner, backend, version, context, limits, policy):
    require(isinstance(context, str) and HEX.fullmatch(context))
    binary = shutil.which(backend.executable)
    require(binary is not None)
    paths = sorted(scanner.glob("image_secrets/*.py")) + [
        scanner / "scan_image_secrets.py", scanner / "scan-image-secrets.sh",
        scanner / "gitleaks.toml", scanner / "SOURCE-MANIFEST.json",
    ]
    return hashlib.sha256(canonical({
        "schema": SCHEMA, "scanner_version": version,
        "scanner": {str(p.relative_to(scanner)): digest_file(p) for p in paths},
        "detector_binary": digest_file(Path(binary)), "detector_version": backend.version,
        "runtime": [list(sys.version_info[:3]), platform.system(), platform.machine()],
        "policy_context": context, "policy": policy, "limits": limits,
        "flags": ["ignore-gitleaks-allow", "max-target-megabytes=0", "max-archive-depth=0", "max-decode-depth=5"],
    })).hexdigest()


def validate_record(record):
    require(isinstance(record, dict) and set(record) == {"diff_id", "coverage", "rows"})
    require(isinstance(record["diff_id"], str) and DIFF_ID.fullmatch(record["diff_id"]))
    coverage, rows = record["coverage"], record["rows"]
    require(isinstance(coverage, list) and len(coverage) <= MAX_INPUTS)
    names = set()
    for number, item in enumerate(coverage):
        require(isinstance(item, dict) and set(item) == {"name", "size", "sha256"})
        require(item["name"] == f"entry-{number:08d}.txt" and type(item["size"]) is int
                and item["size"] >= 0 and isinstance(item["sha256"], str) and HEX.fullmatch(item["sha256"]))
        names.add(item["name"])
    require(isinstance(rows, list) and len(rows) <= MAX_ROWS)
    for row in rows:
        require(isinstance(row, dict) and set(row) == {"input", "rule", "candidate_sha256", "line"})
        require(isinstance(row["input"], str) and row["input"] in names
                and isinstance(row["rule"], str) and re.fullmatch(r"gitleaks\.[a-zA-Z0-9_.-]+", row["rule"])
                and isinstance(row["candidate_sha256"], str) and HEX.fullmatch(row["candidate_sha256"])
                and type(row["line"]) is int and row["line"] >= 0)


class LayerCache:
    def __init__(self, identity, source=None):
        require(isinstance(identity, str) and HEX.fullmatch(identity))
        self.identity, self.layers = identity, {}
        self.stats = {"hits": 0, "misses": 0, "hit_bytes": 0, "miss_bytes": 0, "withheld": 0}
        if source is not None:
            require(not source.is_symlink() and source.is_file() and source.stat().st_size <= MAX_BYTES)
            with source.open("rb") as stream:
                raw = stream.read(MAX_BYTES + 1)
            require(len(raw) <= MAX_BYTES)
            doc = json_bytes(raw)
            require(isinstance(doc, dict) and set(doc) == {"schema", "identity", "layers"})
            require(doc["schema"] == SCHEMA and isinstance(doc["identity"], str) and HEX.fullmatch(doc["identity"]))
            # A changed scanner/catalog/limit is a miss, never old approval.
            if doc["identity"] != identity:
                return
            require(isinstance(doc["layers"], dict) and len(doc["layers"]) <= MAX_LAYERS)
            total_inputs = total_rows = 0
            for key, record in doc["layers"].items():
                validate_record(record)
                require(key == self.key(record["diff_id"]))
                total_inputs += len(record["coverage"])
                total_rows += len(record["rows"])
            require(total_inputs <= MAX_INPUTS and total_rows <= MAX_ROWS)
            self.layers = doc["layers"]

    def key(self, diff_id):
        require(isinstance(diff_id, str) and DIFF_ID.fullmatch(diff_id))
        return hashlib.sha256(canonical({"identity": self.identity, "diff_id": diff_id})).hexdigest()

    def lookup(self, diff_id, coverage):
        record = self.layers.get(self.key(diff_id))
        kind = "miss" if record is None else "hit"
        self.stats["misses" if record is None else "hits"] += 1
        self.stats[kind + "_bytes"] += sum(item["size"] for item in coverage)
        if record is None:
            return None
        require(record["diff_id"] == diff_id and record["coverage"] == coverage)
        return record["rows"]

    def store(self, diff_id, coverage, rows):
        key = self.key(diff_id)
        others = [value for name, value in self.layers.items() if name != key]
        if (len(others) >= MAX_LAYERS or len(coverage) + sum(len(v["coverage"]) for v in others) > MAX_INPUTS
                or len(rows) + sum(len(v["rows"]) for v in others) > MAX_ROWS):
            self.stats["withheld"] += 1
            return
        record = {"diff_id": diff_id, "coverage": coverage, "rows": rows}
        validate_record(record)
        candidate = dict(self.layers, **{key: record})
        if len(canonical({"schema": SCHEMA, "identity": self.identity, "layers": candidate})) > MAX_BYTES:
            self.stats["withheld"] += 1
            return
        self.layers = candidate

    def save(self, destination):
        require(not destination.is_symlink())
        raw = canonical({"schema": SCHEMA, "identity": self.identity, "layers": self.layers})
        require(len(raw) <= MAX_BYTES)
        destination.parent.mkdir(parents=True, exist_ok=True)
        temporary = destination.with_suffix(".pending")
        with temporary.open("xb") as stream:
            stream.write(raw)
        temporary.replace(destination)
