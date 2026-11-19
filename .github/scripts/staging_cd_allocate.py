"""Allocate component versions from develop ancestry using insert-only successor claims."""
import json
import os
from pathlib import Path

from staging_cd_admission import event_identity
from staging_cd_compatibility import GROUP
from staging_cd_transition import complete_group
from staging_cd_allocate_progress import allocation_progress
from staging_cd_allocation_io import AllocationGitHub as GitHub, AllocationIO, parallel_read
from staging_cd_allocation_snapshot import AllocationConflict, AllocationSnapshot as Reservations, SnapshotAPI
from staging_cd_common import Refusal, matches, require
from staging_cd_github import GitHubNotFound
from staging_cd_receipts import canonical
from staging_cd_reservations import next_reservation
from staging_cd_resolve import output
from staging_cd_runtime import run_private
from staging_cd_versions import DEPLOYABLES, label_level, merged_pr, version_tuple

REFUSAL_CODES = {
    "Reservation owner run could not be read": "RESERVATION_OWNER_UNREADABLE",
    "Reservation run identity is invalid": "RESERVATION_OWNER_INVALID",
    "Reservation run liveness is uncertain": "RESERVATION_OWNER_UNCERTAIN",
    "Reservation run and attempt disagree": "RESERVATION_OWNER_INCONSISTENT",
    "Release queue is invalid": "QUEUE_INVALID",
    "Release change identity is invalid": "CHANGE_INVALID",
    "Source already has conflicting final versions": "FINAL_VERSION_CONFLICT",
    "Source must belong to exactly one merged develop PR": "MERGED_PR_NOT_UNIQUE",
    "Merged PR identity changed": "MERGED_PR_IDENTITY_CHANGED",
    "Merged PR must carry exactly one supported version-upgrade label": "VERSION_LABEL_REQUIRED",
    "Version labels are malformed": "VERSION_LABELS_INVALID",
    "Release source is not on verified develop first-parent history": "SOURCE_HISTORY_INVALID",
    "GitHub API request failed": "GITHUB_READ_FAILED",
    "GitHub API response is invalid": "GITHUB_RESPONSE_INVALID",
    "Version reservation collision limit exceeded": "RESERVATION_COLLISION_LIMIT",
    "Reservation push failed without a collision": "RESERVATION_PUSH_FAILED",
    "Reservation push was not verified": "RESERVATION_VERIFY_FAILED",
    "Terminal reservation settlement is uncertain": "FINAL_SETTLEMENT_UNCERTAIN",
    "Terminal reservation differs from final receipt": "FINAL_SETTLEMENT_CONFLICT",
    "Allocation deadline exceeded": "DEADLINE_EXCEEDED",
    "Allocation read failed": "READ_FAILED",
    "Allocation write settlement is uncertain": "WRITE_UNCERTAIN",
    "Allocation claim owner differs": "RESERVATION_VERIFY_FAILED",
    "Allocation claim has no durable version": "INVENTORY_INCOMPLETE",
    "Allocation reservation collision": "RESERVATION_COLLISION",
    "Allocation tag inventory is incomplete": "INVENTORY_INCOMPLETE",
    "Allocation reservation readback differs": "RESERVATION_VERIFY_FAILED",
    "Allocation reservation readback is unavailable": "RESERVATION_NOT_VISIBLE",
}


def run_allocation(operation):
    # Never echo exception text or private output. Only this closed vocabulary
    # crosses the existing output boundary; unknown failures remain generic.
    reason = "UNEXPECTED_ERROR"

    def guarded(masker):
        nonlocal reason
        try:
            return operation(masker)
        except Refusal as error:
            reason = REFUSAL_CODES.get(str(error), "UNCLASSIFIED_REFUSAL")
            raise

    status = run_private(guarded, None)
    if status:
        print("FAIL allocation " + reason)
    return status


def run_active(api, reservation):
    path = f"actions/runs/{reservation['run_id']}"
    try:
        run = api.request(path + f"/attempts/{reservation['run_attempt']}", allow_not_found=True)
    except GitHubNotFound:
        try:
            api.request(path, allow_not_found=True)
        except GitHubNotFound:
            # ABANDONED-BURNED: retain this reservation in the watermark.
            # Allocation creates a new version; it never resumes this one.
            return False
        except Refusal as error:
            raise Refusal("Reservation owner run could not be read") from error
        raise Refusal("Reservation run and attempt disagree")
    except Refusal as error:
        raise Refusal("Reservation owner run could not be read") from error
    require(isinstance(run, dict) and run.get("id") == reservation["run_id"]
            and run.get("run_attempt") == reservation["run_attempt"]
            and run.get("event") == "push" and run.get("head_branch") == "develop"
            and isinstance(run.get("repository"), dict)
            and run["repository"].get("full_name") == "ChronoAIProject/cma",
            "Reservation run identity is invalid")
    require(run.get("status") in ("queued", "in_progress", "waiting", "pending", "requested", "completed"),
            "Reservation run liveness is uncertain")
    return run["status"] != "completed"


def allocate_queue(queue, api, reservations, run_id, attempt, *, mark=lambda _component, _phase: None):
    require(isinstance(queue, dict) and matches(r"[0-9a-f]{40}", queue.get("head"))
            and isinstance(queue.get("changes"), list), "Release queue is invalid")
    mark("all", "final-inventory")
    finals = api.tags()
    mark("all", "pending-inventory")
    pending = reservations.list()
    mark("all", "history")
    history = api.history()
    jobs, seen = [], set()
    # A durable newer reservation/final makes an older source obsolete without
    # deleting its burned version. Reconcile in ancestry order, never input order.
    for change in queue["changes"]:
        require(isinstance(change, dict), "Release change identity is invalid")
        source, tree, deployables = (change.get(key) for key in ("source", "tree", "deployables"))
        require(matches(r"[0-9a-f]{40}", source) and matches(r"[0-9a-f]{40}", tree)
                and source not in seen and isinstance(deployables, list)
                and all(isinstance(name, str) for name in deployables)
                and deployables == sorted(set(deployables)) and set(deployables).issubset(DEPLOYABLES.values()),
                "Release change identity is invalid")
        seen.add(source)
        history.position(source)
    changes = sorted(queue["changes"], key=lambda change: history.position(change["source"]))
    newest = {name: change["source"] for change in changes for name in change["deployables"]}
    owners, busy = {}, set()
    group_sources = [newest[DEPLOYABLES[key]] for key in GROUP if DEPLOYABLES[key] in newest]
    group_tip = max(group_sources, key=history.position, default=None) if queue.get("contract_transition") else None
    for item in pending:
        if item["component"] not in DEPLOYABLES or DEPLOYABLES[item["component"]] not in newest:
            continue
        # Only duplicate work for the very same source needs an owner lookup.
        # Strictly older owners lose authority when the successor transaction lands.
        if item["source"] != newest[DEPLOYABLES[item["component"]]]:
            continue
        if group_tip and item["component"] in GROUP and item["source"] != group_tip:
            # A sibling source moves only with an actual group publication.
            # Defer it to the atomic group plan without waiting for its writer.
            busy.add(item["component"])
            continue
        owner = (item["run_id"], item["run_attempt"])
        if owner == (run_id, attempt):
            continue
        candidates = [value for value in pending if value["component"] == item["component"]]
        candidates += [tag["receipt"]["identity"] for tag in finals
                       if tag["receipt"]["identity"]["component"] == item["component"]]
        # Use the same durable fence as publication. Queue intent alone cannot
        # revoke a writer between its last check and registry copy.
        if item["source"] not in history.positions or any(
                (value["source"] in history.positions
                 and history.position(value["source"]) > history.position(item["source"]))
                or version_tuple(value["version"]) > version_tuple(item["version"]) for value in candidates):
            continue
        owners.setdefault(owner, []).append(item)

    def owner_active(items):
        item = items[0]
        mark(item["component"], "owner-read")
        return run_active(api, item)

    to_reconcile = []
    final_names = {tag["name"] for tag in finals}
    for items, active in zip(owners.values(), parallel_read(owner_active, owners.values())):
        if active:
            for item in items:
                busy.add(item["component"])
        else:
            for item in items:
                name = DEPLOYABLES[item["component"]]
                if name + "/v" + item["version"] not in final_names:
                    to_reconcile.append(item)
    if to_reconcile:
        settled = parallel_read(reservations.final_after_owner, to_reconcile)
        finals.extend(tag for tag in settled if tag is not None)
    actions, needed_prs = [], {}
    for change in changes:
        source, tree, deployables = (change[key] for key in ("source", "tree", "deployables"))
        if not deployables:
            continue
        published = [tag["receipt"] for tag in finals if tag["target"] == source]
        for component, name in DEPLOYABLES.items():
            if name not in deployables or component in busy:
                continue
            stored = [item for item in pending if item["component"] == component]
            stored += [tag["receipt"]["identity"] for tag in finals
                       if tag["receipt"]["identity"]["component"] == component]
            # Legacy reservations whose sources left history still burn versions,
            # but cannot establish a source ordering or authorize publication.
            if any(item["source"] in history.positions
                   and history.position(item["source"]) > history.position(source) for item in stored):
                continue
            existing = [receipt for receipt in published if receipt["identity"]["component"] == component]
            require(len(existing) <= 1, "Source already has conflicting final versions")
            if existing:
                latest = max((tag["receipt"] for tag in finals if tag["receipt"]["identity"]["component"] == component),
                             key=lambda receipt: history.position(receipt["source"]))
                # History reconciliation skips obsolete deployments. A rerun of
                # the latest publication resumes its exact immutable digest.
                if existing[0] == latest and source == newest[name] and source == queue.get("event_source"):
                    jobs.append({"component": component, "mode": "resume", "receipt": existing[0]})
                continue
            owned = [item for item in pending if item["source"] == source and item["component"] == component]
            if source != newest[name] and owned:
                # SUPERSEDED-BURNED: keep the original immutable reservation.
                # A retry must not burn this obsolete source again.
                continue
            needed_prs.setdefault(source, (component, published))
            actions.append((component, source, tree))

    def read_pr(source):
        component, published = needed_prs[source]
        mark(component, "merged-pr")
        pr = merged_pr(source, api.request)
        return pr["number"], published[0]["identity"]["bump"] if published else label_level(pr)

    provenance = dict(zip(needed_prs, parallel_read(read_pr, needed_prs)))
    for component, source, tree in actions:
        pr, level = provenance[source]
        value = next_reservation(component, source, tree, pr, level, run_id, attempt, finals, pending)
        mark(component, "reserve")
        require(reservations.insert(value), "Allocation reservation collision")
        pending.append(value)
        if source == newest[DEPLOYABLES[component]]:
            jobs.append({"component": component, "mode": "publish", "reservation": value})
    # Intermediate reservations preserve each merged label's version increment,
    # but only the newest source is built. Failed attempts remain burned.
    transition = queue.get("contract_transition")
    complete = complete_group(transition, jobs, changes, api, reservations, finals, pending, run_id, attempt)
    grouped = [{"component": key, **({"contract_transition": transition} if complete and key in GROUP else {}),
                "tasks": canonical([job for job in jobs if job["component"] == key])}
               for key in DEPLOYABLES if any(job["component"] == key for job in jobs)]
    mark("all", "planned")
    return {"include": grouped}


def main():
    env = os.environ
    def operation(_masker):
        event_identity(env)
        require(env["GITHUB_EVENT_NAME"] == "push", "Version allocation only accepts develop pushes")
        io = AllocationIO()
        queue = json.loads(env["CD_QUEUE"])
        require(isinstance(queue, dict) and matches(r"[0-9a-f]{40}", queue.get("event_source"))
                and queue["event_source"] == env["GITHUB_SHA"], "Release queue is invalid")
        matrix = allocate_current(queue, env, io, mark)
        io.remaining()
        mark("all", "complete")
        output(env, "matrix", canonical(matrix))
        output(env, "changed", "true" if matrix["include"] else "false")
        return True
    with allocation_progress() as mark:
        return run_allocation(operation)


def allocate_current(queue, env, io, mark):
    # A lost race proves another atomic allocation completed. Only then may we
    # plan a distinct transaction, from a fresh history and complete inventory.
    require(matches(r"[0-9a-f]{40}", queue.get("head")) and isinstance(queue.get("changes"), list),
            "Release queue is invalid")
    if queue["event_source"] != queue["head"]:
        mark("all", "stale-source")
        return {"include": []}
    api = GitHub(env.get("GH_TOKEN"), io)
    while True:
        io.remaining()
        reservations = Reservations(api, env.get("GH_TOKEN"), Path.cwd(), io, mark=mark)
        if not reservations.load(event_source=queue["event_source"]):
            mark("all", "stale-source")
            return {"include": []}
        matrix = allocate_queue(queue, SnapshotAPI(api, reservations), reservations,
                                int(env["GITHUB_RUN_ID"]), int(env["GITHUB_RUN_ATTEMPT"]), mark=mark)
        try:
            reservations.commit()
        except AllocationConflict:
            mark("all", "replan")
            continue
        return matrix


if __name__ == "__main__":
    raise SystemExit(main())
