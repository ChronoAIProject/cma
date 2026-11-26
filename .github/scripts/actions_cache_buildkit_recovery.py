"""Recover aged native BuildKit blobs under the verified retired-writer floor."""

import json
import os
import sys
from dataclasses import replace
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


def recover(api, reader, revision, now, *, retire_indexes=False):
    api.require_live_policy(revision)
    initial = sum(row.size for row in api.entries())
    result = {"before_bytes": initial, "budget_bytes": BUDGET, "candidate_bytes": 0,
              "confirmed_deleted_bytes": 0, "status": "under-target", "confirmed_retired_indexes": 0,
              "superseded_index_count": 0, "next_index_batch_count": 0,
              "projected_newly_unreachable_bytes": 0, "projected_candidate_bytes_after_index_retirement": 0}
    retired_ids = set()
    if initial >= BUDGET:
        result["status"] = "target-unmet"
        if not idle(api):
            result["status"] = "reader-held"
        else:
            try:
                observation = observe(api, reader, revision, now, retirement_target=initial - BUDGET + 1)
            except ProofError as error:
                result.update(status="proof-incomplete", proof_phase=error.phase, error_class=error.cause_type)
                candidates = []
                retiring_indexes = False
            else:
                pinned, candidates = observation.pinned, observation.candidates
                result.update(candidate_bytes=sum(row.size for row in candidates),
                              superseded_index_count=len(observation.superseded),
                              next_index_batch_count=len(observation.retirement_batch),
                              projected_newly_unreachable_bytes=observation.projected_newly_unreachable_bytes,
                              projected_candidate_bytes_after_index_retirement=observation.projected_candidate_bytes)
                # Layers made unreachable here are considered only by a later,
                # independent observation against the surviving indexes.
                retiring_indexes = retire_indexes and not candidates and bool(observation.retirement_batch)
                if retiring_indexes:
                    candidates = observation.retirement_batch
            remaining = sum(row.size for row in api.entries())
            for candidate in candidates:
                if remaining < BUDGET:
                    break
                try:
                    api.require_live_policy(revision)
                    if not idle(api):
                        result["status"] = "reader-held"
                        break
                    if indexes(entries_with_prefix(api, INDEX_PREFIX)) != pinned:
                        result["status"] = "proof-changed"
                        break
                    visible = api.visible(candidate)
                    if retiring_indexes and visible:
                        visible = replace(visible, accessed=visible.created)
                    if visible != candidate:
                        result["status"] = "proof-changed"
                        break
                    api.require_live_policy(revision)
                except (OSError, ValueError, RuntimeError, SubprocessError):
                    result["status"] = "proof-changed"
                    break
                action = "native-index-delete-intent" if retiring_indexes else "native-blob-delete-intent"
                print(json.dumps({"action": action, "id": candidate.id,
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
                if retiring_indexes:
                    retired_ids.add(candidate.id)
                    pinned = {index_id: row for index_id, row in pinned.items() if index_id != candidate.id}
                else:
                    result["confirmed_deleted_bytes"] += candidate.size
                remaining = sum(row.size for row in api.entries())
    final = api.entries()
    remaining_ids = {row.id for row in final}
    result["confirmed_retired_indexes"] = len(retired_ids - remaining_ids)
    result["after_bytes"] = sum(row.size for row in final)
    result["bytes_to_admit"] = max(0, result["after_bytes"] - BUDGET + 1)
    if "uncertain_id" in result:
        result["uncertain_id_absent"] = result["uncertain_id"] not in remaining_ids
    elif result["status"] in {"under-target", "target-unmet"}:
        if result["after_bytes"] < BUDGET:
            result["status"] = "recovered" if initial >= BUDGET else "under-target"
        elif retired_ids:
            result["status"] = ("indexes-retired" if retired_ids.isdisjoint(remaining_ids)
                                else "retirement-awaiting-readback")
        else:
            result["status"] = "target-unmet"
    return result


def main(env):
    if not authorized(env):
        raise ValueError("native BuildKit recovery requires verified writer and reader floor")
    api, reader = client(env)
    result = recover(api, reader, env["GITHUB_SHA"], datetime.now(timezone.utc),
                     retire_indexes=env.get("CD_LEGACY_BUILDKIT_INDEX_RETIREMENT") == "enabled")
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
