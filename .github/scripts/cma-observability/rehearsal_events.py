#!/usr/bin/env python3
"""Standalone rehearsal watcher: explicit namespace UID, read-only metadata."""
import json
import os
import subprocess
import tempfile

from watch_hook_events import MAX_BYTES, parser, read_events, run


def verify_namespace(kubectl, context, namespace, expected_uid):
    command = [kubectl, "--context", context, "get", "namespace", namespace,
               "--request-timeout=5s", "-o", "json"]
    with tempfile.TemporaryFile() as output, tempfile.TemporaryFile() as error:
        try:
            result = subprocess.run(command, stdout=output, stderr=error, timeout=8, check=False)
        except subprocess.TimeoutExpired as exc:
            raise ValueError("events_namespace_timeout") from exc
        if result.returncode or output.tell() > MAX_BYTES:
            raise ValueError("events_namespace_unavailable")
        output.seek(0)
        row = json.load(output)
    if not isinstance(row, dict) or not isinstance(row.get("metadata"), dict):
        raise ValueError("events_namespace_invalid")
    meta = row["metadata"]
    if (row.get("kind") != "Namespace" or meta.get("name") != namespace
            or meta.get("uid") != expected_uid or meta.get("deletionTimestamp")):
        raise ValueError("events_namespace_identity_changed")


def main():
    os.umask(0o077)
    p = parser()
    p.description = __doc__
    p.set_defaults(interval=2, duration=7200, lookback=120)
    p.add_argument("--namespace-uid", required=True,
                   help="Fresh namespace UID from the executor's rehearsal binding")
    args = p.parse_args()
    try:
        if len({scope["namespace"] for scope in args.target}) != 1 or args.replay:
            raise ValueError("events_rehearsal_requires_one_live_namespace")
        if not args.namespace_uid or len(args.namespace_uid) > 128:
            raise ValueError("events_namespace_uid_invalid")
        def reader(kubectl, context, namespace):
            verify_namespace(kubectl, context, namespace, args.namespace_uid)
            return read_events(kubectl, context, namespace)
        return run(args, reader=reader)
    except KeyboardInterrupt:
        return 130
    except (ValueError, OSError):
        print(json.dumps({"schema": "cma-hook-watch-gap-v1", "severity": "critical",
                          "code": "watcher_failed", "delivery": "NOT_SENT"}), flush=True)
        return 2


if __name__ == "__main__":
    raise SystemExit(main())
