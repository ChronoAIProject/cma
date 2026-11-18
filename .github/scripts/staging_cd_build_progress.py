"""Summarize private BuildKit progress without publishing commands or payloads."""
import datetime
import hashlib
import json
import math
import re

CATEGORIES = ("rust-compile", "other-build", "export")


def summarize(raw):
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
    for row in vertices.values():
        name = row.get("name", "")
        if not isinstance(name, str):
            continue
        if row.get("cached") is True:
            cached += 1
        try:
            start, end = (datetime.datetime.fromisoformat(row[key].replace("Z", "+00:00")).timestamp()
                          for key in ("started", "completed"))
        except (KeyError, ValueError, TypeError, AttributeError):
            continue
        if not all(math.isfinite(t) for t in (start, end)) or end < start:
            continue
        category = "rust-compile" if re.search(r"\bcargo\s+build\b", name) else (
            "export" if name.startswith("exporting ") else "other-build")
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
    return {"complete": bool(complete), "vertices": len(vertices), "cached_vertices": cached,
            "category_seconds": durations}
