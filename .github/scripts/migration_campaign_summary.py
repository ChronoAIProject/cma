#!/usr/bin/env python3
"""Publish only the identities and verdicts of a private campaign record.

The repository is public, so the raw kind evidence never leaves the runner.
This prints its sha256, the runner outcome, and per-stage fixture and
catalog verdicts: no sample, manifest, log or credential material.
"""
import hashlib
import json
from pathlib import Path
import sys

PASSING = {"UNVERIFIED"}  # the runner's success outcome pending coverage review


def summary(path, fixture):
    data = Path(path).read_bytes()
    value = json.loads(data)
    lines = [f"### Migration campaign: {fixture}", "",
             f"- Outcome: `{value.get('outcome')}`",
             f"- Evidence sha256: `{hashlib.sha256(data).hexdigest()}`",
             f"- Cluster removed: `{(value.get('cleanup') or {}).get('cluster_absent')}`", "",
             "| Stage | Reader source | Fixture | Collections | Checks |", "|---|---|---|---|---|"]
    for row in value.get("fixtures", []):
        checks = row.get("checks") or {}
        passed = sum(1 for result in checks.values() if result)
        lines.append(f"| {row.get('stage')} | `{str(row.get('source'))[:12]}` | {row.get('outcome')} "
                     f"| {len(row.get('collections') or [])} | {passed}/{len(checks)} |")
    comparisons = [row.get("outcome") for row in value.get("catalog_comparisons", [])]
    lines += ["", f"- Catalog comparisons: {', '.join(map(str, comparisons)) or 'none'}"]
    return "\n".join(lines) + "\n", value.get("outcome") in PASSING and \
        (value.get("cleanup") or {}).get("cluster_absent") is True


def main(argv=None):
    argv = sys.argv[1:] if argv is None else argv
    text, ok = summary(argv[0], argv[1])
    sys.stdout.write(text)
    raise SystemExit(0 if ok else 1)


if __name__ == "__main__":
    main()
