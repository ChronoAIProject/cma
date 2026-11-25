#!/usr/bin/env python3
"""Publish only the identities and verdicts of a private campaign record.

The repository is public, so the raw kind evidence never leaves the runner.
This prints its sha256, the runner outcome, and per-stage fixture and
catalog verdicts: no sample, manifest, log or credential material.
"""
import hashlib
import json
from pathlib import Path
import re
import sys

PASSING = {"UNVERIFIED"}  # the runner's success outcome pending coverage review
# Only lowercase resource-like words of a failed command are published: no
# paths, no key=value pairs, no flag values, nothing a credential could be.
WORD = re.compile(r"^[a-z][a-z0-9./:_-]{0,80}$")
VALUED_FLAGS = {"--kubeconfig", "--context", "-o", "--output", "--request-timeout", "--config", "--name"}


def failed_command(commands):
    for row in reversed(commands or []):
        if row.get("exit") == 0 and not row.get("timed_out"):
            continue
        argv = [str(token) for token in row.get("argv") or []]
        words, skip = [], False
        for token in argv[1:]:
            if skip:
                skip = False
            elif token in VALUED_FLAGS:
                skip = True
            elif not token.startswith("-") and WORD.fullmatch(token):
                words.append(token)
            if len(words) == 4:
                break
        program = Path(argv[0]).name if argv and WORD.fullmatch(Path(argv[0]).name) else "command"
        status = "timed out" if row.get("timed_out") else f"exit {row.get('exit')}"
        return f"`{' '.join([program] + words)}` ({status})"
    return None


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
    lines += ["", f"- Catalog comparisons: {', '.join(map(str, comparisons)) or 'none'}",
              f"- Boots: {', '.join(str(row.get('stage')) for row in value.get('boots', [])) or 'none'}"]
    if value.get("outcome") not in PASSING:
        # Runner refusals are fixed messages it composes itself.
        reason = str(value.get("reason", "unknown")).splitlines()[0][:200]
        lines.append(f"- Reason: {reason}")
        failed = failed_command(value.get("commands"))
        if failed:
            lines.append(f"- Last failed command: {failed}")
    return "\n".join(lines) + "\n", value.get("outcome") in PASSING and \
        (value.get("cleanup") or {}).get("cluster_absent") is True


def main(argv=None):
    argv = sys.argv[1:] if argv is None else argv
    text, ok = summary(argv[0], argv[1])
    sys.stdout.write(text)
    raise SystemExit(0 if ok else 1)


if __name__ == "__main__":
    main()
