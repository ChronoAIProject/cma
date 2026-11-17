"""Allocate under the short global lock; build and deployment use no global lock."""
import json
import os
from pathlib import Path

from staging_cd_admission import event_identity
from staging_cd_common import matches, require
from staging_cd_github import GitHub
from staging_cd_receipts import canonical
from staging_cd_reservations import Reservations, next_reservation, reserve
from staging_cd_resolve import output
from staging_cd_runtime import run_private
from staging_cd_versions import DEPLOYABLES, label_level, merged_pr, version_tuple


def run_active(api, reservation):
    run = api.request(f"actions/runs/{reservation['run_id']}/attempts/{reservation['run_attempt']}")
    require(isinstance(run, dict) and run.get("id") == reservation["run_id"]
            and run.get("run_attempt") == reservation["run_attempt"]
            and run.get("event") == "push" and run.get("head_branch") == "develop"
            and isinstance(run.get("repository"), dict)
            and run["repository"].get("full_name") == "ChronoAIProject/cma",
            "Reservation run identity is invalid")
    require(run.get("status") in ("queued", "in_progress", "waiting", "pending", "requested", "completed"),
            "Reservation run liveness is uncertain")
    return run["status"] != "completed"


def allocate_queue(queue, api, reservations, run_id, attempt):
    require(isinstance(queue, dict) and matches(r"[0-9a-f]{40}", queue.get("head"))
            and isinstance(queue.get("changes"), list), "Release queue is invalid")
    finals, pending = api.tags(), reservations.list()
    jobs, seen = [], set()
    for change in queue["changes"]:
        source, tree, deployables = change["source"], change["tree"], change["deployables"]
        require(matches(r"[0-9a-f]{40}", source) and matches(r"[0-9a-f]{40}", tree)
                and source not in seen and isinstance(deployables, list)
                and deployables == sorted(set(deployables)) and set(deployables).issubset(DEPLOYABLES.values()),
                "Release change identity is invalid")
        seen.add(source)
        if not deployables:
            continue
        pr, level = None, None
        published = [tag["receipt"] for tag in finals if tag["target"] == source]
        for component, name in DEPLOYABLES.items():
            if name not in deployables:
                continue
            existing = [receipt for receipt in published if receipt["identity"]["component"] == component]
            require(len(existing) <= 1, "Source already has conflicting final versions")
            if existing:
                latest = max((tag["receipt"] for tag in finals if tag["receipt"]["identity"]["component"] == component),
                             key=lambda receipt: version_tuple(receipt["identity"]["version"]))
                # History reconciliation skips obsolete deployments. A rerun of
                # the latest publication resumes its exact immutable digest.
                if existing[0] == latest and source == queue.get("event_source"):
                    jobs.append({"component": component, "mode": "resume", "receipt": existing[0]})
                continue
            owned = [item for item in pending if item["source"] == source and item["component"] == component]
            if any((item["run_id"], item["run_attempt"]) != (run_id, attempt) and run_active(api, item) for item in owned):
                continue
            if pr is None:
                pr = merged_pr(source, api.request)
                level = published[0]["identity"]["bump"] if published else label_level(pr)
            def next_value(current, reserved):
                return next_reservation(component, source, tree, pr["number"], level, run_id, attempt,
                                        current, reserved)
            value = reserve(next_value, reservations.insert, lambda: (api.tags(), reservations.list()))
            pending.append(value)
            jobs.append({"component": component, "mode": "publish", "reservation": value})
    # One component chain runs in source order; independent components remain
    # parallel. A canceled/failed chain leaves reservations burned for retries.
    grouped = [{"component": key, "tasks": canonical([job for job in jobs if job["component"] == key])}
               for key in DEPLOYABLES if any(job["component"] == key for job in jobs)]
    return {"include": grouped}


def main():
    env = os.environ
    def operation(_masker):
        event_identity(env)
        require(env["GITHUB_EVENT_NAME"] == "push", "Version allocation only accepts develop pushes")
        api = GitHub(env.get("GH_TOKEN"))
        reservations = Reservations(api, env.get("GH_TOKEN"), Path.cwd())
        matrix = allocate_queue(json.loads(env["CD_QUEUE"]), api, reservations,
                                int(env["GITHUB_RUN_ID"]), int(env["GITHUB_RUN_ATTEMPT"]))
        output(env, "matrix", canonical(matrix))
        output(env, "changed", "true" if matrix["include"] else "false")
        return True
    return run_private(operation, None)


if __name__ == "__main__":
    raise SystemExit(main())
