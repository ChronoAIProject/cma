"""Recover aged native BuildKit blobs under the verified retired-writer floor."""

import json
import os
import sys
from datetime import datetime, timezone
from pathlib import Path
from subprocess import SubprocessError

from actions_cache_buildkit_inventory import (
    INDEX_PREFIX, ProofError, client, entries_with_prefix, indexes, observe,
)
from actions_cache_retention import BUDGET
from actions_cache_sweep import authorized_completion


def authorized(env):
    # An operator attests that every historical native writer and in-flight
    # reader has drained, including old workflow reruns. The job's lock covers
    # current participating cache maintenance throughout this operation.
    if (env.get("CD_LEGACY_BUILDKIT_GC") != "enabled"
            or env.get("CD_LAYER_CACHE_WRITER_FLOOR") != "verified"
            or not authorized_completion(env)):
        return False
    event = json.loads(Path(env["GITHUB_EVENT_PATH"]).read_text())
    return event["workflow_run"]["path"] == ".github/workflows/staging-cd.yml"


def idle(api):
    return all(run["id"] == api.run_id for run in api.active_runs())


def recover(api, reader, revision, now):
    api.require_live_policy(revision)
    initial = sum(row.size for row in api.entries())
    result = {"before_bytes": initial, "budget_bytes": BUDGET, "candidate_bytes": 0,
              "confirmed_deleted_bytes": 0, "status": "under-target"}
    if initial >= BUDGET:
        result["status"] = "target-unmet"
        if not idle(api):
            result["status"] = "reader-held"
        else:
            try:
                pinned, candidates, _, _ = observe(api, reader, revision, now)
            except ProofError as error:
                result.update(status="proof-incomplete", proof_phase=error.phase, error_class=error.cause_type)
                candidates = []
            result["candidate_bytes"] = sum(row.size for row in candidates)
            remaining = sum(row.size for row in api.entries())
            for candidate in candidates:
                if remaining < BUDGET:
                    break
                try:
                    api.require_live_policy(revision)
                    if not idle(api):
                        result["status"] = "reader-held"
                        break
                    if indexes(entries_with_prefix(api, INDEX_PREFIX)) != pinned or api.visible(candidate) != candidate:
                        result["status"] = "proof-changed"
                        break
                    api.require_live_policy(revision)
                except (OSError, ValueError, RuntimeError, SubprocessError):
                    result["status"] = "proof-changed"
                    break
                print(json.dumps({"action": "native-blob-delete-intent", "id": candidate.id,
                                  "bytes": candidate.size}), flush=True)
                try:
                    removed = api.delete(candidate.id)
                except (OSError, RuntimeError, SubprocessError):
                    # A lost response never authorizes more removals. Resolve
                    # this numeric identity with the final inventory.
                    result["uncertain_id"] = candidate.id
                    result["status"] = "mutation-uncertain"
                    break
                if not removed:
                    result["uncertain_id"] = candidate.id
                    result["status"] = "candidate-disappeared"
                    break
                result["confirmed_deleted_bytes"] += candidate.size
                remaining = sum(row.size for row in api.entries())
    final = api.entries()
    result["after_bytes"] = sum(row.size for row in final)
    result["bytes_to_admit"] = max(0, result["after_bytes"] - BUDGET + 1)
    if "uncertain_id" in result:
        result["uncertain_id_absent"] = all(row.id != result["uncertain_id"] for row in final)
    elif result["status"] in {"under-target", "target-unmet"}:
        if result["after_bytes"] < BUDGET:
            result["status"] = "recovered" if initial >= BUDGET else "under-target"
        else:
            result["status"] = "target-unmet"
    return result


def main(env):
    if not authorized(env):
        raise ValueError("native BuildKit recovery requires verified writer and reader floor")
    api, reader = client(env)
    result = recover(api, reader, env["GITHUB_SHA"], datetime.now(timezone.utc))
    print(json.dumps(result), flush=True)
    if env.get("GITHUB_STEP_SUMMARY"):
        with Path(env["GITHUB_STEP_SUMMARY"]).open("a") as summary:
            summary.write(f"### Native BuildKit recovery — {result['status']}\n\n"
                          f"Before {result['before_bytes']} bytes. After {result['after_bytes']} bytes. "
                          f"Additional bytes required for admission {result['bytes_to_admit']}.\n")
    return 0 if result["status"] in {"under-target", "recovered"} else 3


if __name__ == "__main__":
    try:
        sys.exit(main(os.environ))
    except ProofError as error:
        print(f"Native BuildKit recovery failed at {error.phase} ({error.cause_type})", file=sys.stderr)
        sys.exit(1)
    except Exception as error:
        print(f"Native BuildKit recovery failed ({type(error).__name__})", file=sys.stderr)
        sys.exit(1)
