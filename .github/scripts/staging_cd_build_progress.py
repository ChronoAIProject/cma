"""Summarize private BuildKit progress without publishing commands or payloads."""
import datetime
import hashlib
import json
import math
import re

from staging_cd_test_failures import failed_tests

CATEGORIES = ("rust-compile", "dependency-compile", "go-compile", "media-compile",
              "cache-import", "cache-export", "archive-export", "other-build")
# A Dockerfile step is named "[<stage> <n>/<total>] <instruction>". The bracket
# holds only the public stage name and position; the instruction is withheld.
STEP = re.compile(r"\[([A-Za-z0-9][A-Za-z0-9_.-]{0,63}) ([1-9][0-9]{0,3})/([1-9][0-9]{0,3})\] ")
# BuildKit's own vertices for base images and the build context carry no command.
INTERNAL = (("[internal] load metadata for ", "base-image-metadata"),
            ("[internal] load build context", "build-context"),
            ("[internal] load build definition", "build-definition"),
            ("[internal] load .dockerignore", "build-context"))


def summarize(raw, dockerfile=None):
    """Fixed facts about one build; failing test names need the Dockerfile that was built."""
    if len(raw) > 64 * 1024**2:
        return {"complete": False, "reason": "progress-size-limit"}
    vertices = {}
    for line in raw.splitlines():
        try:
            event = json.loads(line)
        except (ValueError, TypeError):
            continue
        if not isinstance(event, dict):
            continue
        records = event.get("vertexes", [event])
        if not isinstance(records, list):
            continue
        for row in records:
            if not isinstance(row, dict):
                continue
            key = row.get("digest", row.get("id"))
            if not isinstance(key, str) or not 0 < len(key) <= 512:
                continue
            # Exporter vertices can use textual IDs. Keep only an opaque map key;
            # neither those IDs nor their command-bearing names reach output.
            vertices.setdefault(hashlib.sha256(key.encode()).hexdigest(), {}).update(row)
    intervals = {name: [] for name in CATEGORIES}
    cached = 0
    failed = []
    for row in vertices.values():
        name = row.get("name", "")
        if not isinstance(name, str):
            continue
        if row.get("error"):
            # Cancelled siblings also carry an error; the earliest one is the cause.
            failed.append((str(row.get("completed") or ""), failed_step(name), row.get("digest")))
        if row.get("cached") is True:
            cached += 1
        try:
            start, end = (datetime.datetime.fromisoformat(row[key].replace("Z", "+00:00")).timestamp()
                          for key in ("started", "completed"))
        except (KeyError, ValueError, TypeError, AttributeError):
            continue
        if not all(math.isfinite(t) for t in (start, end)) or end < start:
            continue
        if re.search(r"\bcargo\s+chef\s+cook\b", name):
            category = "dependency-compile"
        elif re.search(r"\bcargo\s+build\b", name):
            category = "rust-compile"
        elif re.search(r"\bgo\s+build\b", name):
            category = "go-compile"
        elif name.startswith("[ffmpeg-build ") and "./configure " in name:
            category = "media-compile"
        elif name.startswith("importing cache"):
            category = "cache-import"
        elif name.startswith("exporting cache"):
            category = "cache-export"
        elif name.startswith("exporting "):
            category = "archive-export"
        else:
            category = "other-build"
        intervals[category].append((start, end))
    # Vertices execute in parallel. Union each category's intervals rather than
    # adding overlapping durations; categories can still overlap each other.
    durations = {}
    for category, spans in intervals.items():
        merged = []
        for start, end in sorted(spans):
            if merged and start <= merged[-1][1]:
                merged[-1][1] = max(merged[-1][1], end)
            else:
                merged.append([start, end])
        durations[category] = round(sum(end - start for start, end in merged), 3)
    complete = bool(vertices) and any(row.get("completed") for row in vertices.values())
    complete = complete and all(not row.get("started") or row.get("completed") for row in vertices.values())
    result = {"complete": bool(complete), "vertices": len(vertices), "cached_vertices": cached,
              "category_seconds": durations}
    if failed:
        # Name where the build stopped so its owner can open that Dockerfile
        # step; never the instruction text, its arguments or the error message.
        ordered = sorted(failed, key=lambda item: item[:2])
        result["failed_steps"] = list(dict.fromkeys(label for _, label, _ in ordered))[:8]
        # A failed test stage also names its failing tests: only names matched
        # by fixed failure patterns, never its log lines or error messages.
        tests = failed_tests(raw, [(label, digest) for _, label, digest in ordered], dockerfile) if dockerfile else []
        if tests:
            result["failed_tests"] = tests
    return result


def failed_step(name):
    match = STEP.match(name)
    if match:
        return f"{match[1]} {match[2]}/{match[3]}"
    for prefix, label in INTERNAL:
        if name.startswith(prefix):
            return label
    return "other"
