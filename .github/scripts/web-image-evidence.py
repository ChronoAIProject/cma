#!/usr/bin/env python3
"""Stage at most 128 MiB for upload, prioritizing diagnostics over checkpoints."""
import argparse
import json
from pathlib import Path
import sys

MAX_BYTES = 128 * 1024 * 1024
MAX_FILE_BYTES = 32 * 1024 * 1024
MANIFEST_RESERVE = 1024 * 1024
MAX_FILES = 2000


def copy_bounded(source, target, limit):
    """Bound bytes actually read, even if an artifact grows after stat()."""
    copied = 0
    with source.open("rb") as incoming, target.open("xb") as outgoing:
        while True:
            chunk = incoming.read(min(1024 * 1024, limit - copied + 1))
            if not chunk:
                return copied
            if copied + len(chunk) > limit:
                break
            outgoing.write(chunk)
            copied += len(chunk)
    target.unlink()
    return None


def priority(path):
    if path.suffix in {".json", ".jsonl"}:
        return 0
    if path.name == "trace.zip":
        return 1
    if path.name.startswith("test-failed-"):
        return 2
    return 3


def stage(source, destination, *, limit=MAX_BYTES, file_limit=MAX_FILE_BYTES):
    source, destination = source.resolve(), destination.resolve()
    if source == destination or source in destination.parents or destination in source.parents:
        raise ValueError("Evidence staging must use a separate, fresh directory")
    if limit <= MANIFEST_RESERVE or file_limit <= 0:
        raise ValueError("Evidence byte limits must reserve room for the manifest")
    destination.mkdir(parents=True, exist_ok=False)
    manifest = {"max_bytes": limit, "max_file_bytes": file_limit,
                "priority": ["json/jsonl", "trace.zip", "failure screenshots", "checkpoints"],
                "source_missing": not source.is_dir(), "copied_bytes": 0,
                "omitted_files": 0, "omitted_bytes": 0, "files": []}
    paths = sorted(source.rglob("*"), key=lambda p: (priority(p), str(p)))
    for path in paths:
        relative = path.relative_to(source)
        # Match upload-artifact's existing hidden-file exclusion. Never follow links.
        if path.is_symlink() or any(part.startswith(".") for part in relative.parts):
            continue
        if not path.is_file():
            continue
        size = path.stat().st_size
        reason = None
        if len(manifest["files"]) >= MAX_FILES:
            reason = "inventory_limit"
        elif len(str(relative).encode()) > 512:
            reason = "path_limit"
        elif size > file_limit:
            reason = "file_limit"
        elif manifest["copied_bytes"] + size > limit - MANIFEST_RESERVE:
            reason = "total_limit"
        if not reason:
            target = destination / relative
            target.parent.mkdir(parents=True, exist_ok=True)
            copied = copy_bounded(path, target, min(file_limit,
                                  limit - MANIFEST_RESERVE - manifest["copied_bytes"]))
            if copied is None:
                reason = "grew_beyond_limit"
                size = path.stat().st_size
            else:
                size = copied
                manifest["copied_bytes"] += copied
        if reason:
            manifest["omitted_files"] += 1
            manifest["omitted_bytes"] += size
        if len(manifest["files"]) < MAX_FILES and len(str(relative).encode()) <= 512:
            manifest["files"].append({"path": str(relative), "bytes": size,
                                      "disposition": reason or "retained"})
    encoded = (json.dumps(manifest, separators=(",", ":")) + "\n").encode()
    if len(encoded) > MANIFEST_RESERVE:
        # The inventory is bounded independently of payload files, even with long names.
        manifest["files"] = []
        manifest["inventory_omitted"] = "manifest_byte_limit"
        encoded = (json.dumps(manifest) + "\n").encode()
    (destination / "artifact-budget.json").write_bytes(encoded)
    assert manifest["copied_bytes"] + len(encoded) <= limit
    print(json.dumps({key: value for key, value in manifest.items() if key != "files"}))
    return not manifest["source_missing"]


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("source", type=Path)
    parser.add_argument("destination", type=Path)
    args = parser.parse_args()
    sys.exit(0 if stage(args.source, args.destination) else 1)
