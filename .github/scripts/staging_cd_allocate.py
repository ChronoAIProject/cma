"""Allocate component versions from develop ancestry using insert-only successor claims."""
import json
import os
from pathlib import Path
import random
import sys
import time

import ci_retry
from ci_outcome import error_site, report
from staging_cd_admission import event_identity
from staging_cd_baselines import durable_baselines
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
from staging_cd_promotion import available_proofs, proven_components
from staging_cd_versions import DEPLOYABLES, label_level, merged_pr, source_is_retired, version_tuple

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
    "Merged PR has conflicting or unsupported version-upgrade labels": "VERSION_LABEL_REQUIRED",
    "Version labels are malformed": "VERSION_LABELS_INVALID",
    "Release source is not on verified develop first-parent history": "SOURCE_HISTORY_INVALID",
    # Split by its cause in github_read_code; this is the code of no known cause.
    "GitHub API request failed": "GITHUB_READ_FAILED",
    "GitHub API response is invalid": "GITHUB_RESPONSE_INVALID",
    "GitHub read budget expired": "DEADLINE_EXCEEDED",
    "Version reservation collision limit exceeded": "RESERVATION_COLLISION_LIMIT",
    "Reservation push failed without a collision": "RESERVATION_PUSH_FAILED",
    "Reservation push was not verified": "RESERVATION_VERIFY_FAILED",
    "Reservation push was rejected": "RESERVATION_PUSH_REJECTED",
    "Terminal reservation settlement is uncertain": "FINAL_SETTLEMENT_UNCERTAIN",
    "Terminal reservation differs from final receipt": "FINAL_SETTLEMENT_CONFLICT",
    "Allocation deadline exceeded": "DEADLINE_EXCEEDED",
    "Allocation read failed": "READ_FAILED",
    "Allocation write settlement is uncertain": "WRITE_UNCERTAIN",
    "Allocation claim owner differs": "INVENTORY_CLAIM_CONFLICT",
    "Allocation claim has no durable version": "INVENTORY_CLAIM_ORPHANED",
    "Allocation reservation collision": "RESERVATION_COLLISION",
    "Allocation planned a duplicate reservation": "RESERVATION_PLAN_INVALID",
    "Unknown reservation component": "RESERVATION_PLAN_INVALID",
    "Allocation tag inventory is incomplete": "INVENTORY_INCOMPLETE",
    "Develop history has not reached the push": "DEVELOP_ADVERTISEMENT_UNSETTLED",
    "Develop ancestry could not be verified": "DEVELOP_ADVERTISEMENT_UNSETTLED",
    "Allocation tag inventory exceeds 16 MiB": "INVENTORY_TOO_LARGE",
    "Allocation reservation readback differs": "RESERVATION_VERIFY_FAILED",
    "Allocation reservation readback is unavailable": "RESERVATION_NOT_VISIBLE",
}
FALLBACK_CODES = frozenset({"UNEXPECTED_ERROR", "UNCLASSIFIED_REFUSAL"})
# Codes a failed GitHub read takes from its cause rather than its message.
GITHUB_READ_CAUSES = frozenset({"GITHUB_READ_TRANSIENT", "GITHUB_READ_DENIED"})
# The codes ci_outcomes.json catalogues as transient; a test keeps them equal.
# Such a refusal stops allocation at its settlement gate, the step catch-up
# re-runs a head run for, once. Every other refusal fails the allocation step.
TRANSIENT_CODES = frozenset({
    "RESERVATION_OWNER_UNREADABLE", "RESERVATION_OWNER_UNCERTAIN", "MERGED_PR_IDENTITY_CHANGED",
    "VERSION_LABELS_INVALID", "GITHUB_READ_TRANSIENT", "GITHUB_RESPONSE_INVALID", "RESERVATION_COLLISION_LIMIT",
    "RESERVATION_PUSH_FAILED", "RESERVATION_VERIFY_FAILED", "FINAL_SETTLEMENT_UNCERTAIN", "DEADLINE_EXCEEDED",
    "READ_FAILED", "WRITE_UNCERTAIN", "INVENTORY_INCOMPLETE", "INVENTORY_CLAIM_ORPHANED", "RESERVATION_COLLISION",
    "RESERVATION_NOT_VISIBLE", "DEVELOP_ADVERTISEMENT_UNSETTLED",
})
# A transient refusal raised before the reservation push started left nothing
# on the remote, so the whole allocation runs once more in the same job, with
# a fresh budget and inventory after a short jittered wait, or longer when
# GitHub asked for up to the longest wait. A longer request, such as a spent
# primary rate limit, is left to the gate and catch-up: an early rerun would
# only repeat every tag read. Two attempts and the wait take at most 260 s of
# the allocate job's 25 minute timeout.
ALLOCATION_ATTEMPTS = 2
RETRY_WAIT_SECONDS = (10, 20)
# WRITE_UNCERTAIN says a write may have committed. It never reruns
# allocation, whatever the push flag says.
RETRIED_CODES = TRANSIENT_CODES - {"WRITE_UNCERTAIN"}


def github_read_code(error):
    """Name a failed GitHub read by the cause its refusal carries."""
    if ci_retry.classify(error) == ci_retry.TRANSIENT:
        # A 5xx, 408 or 429 response, a rate-limited 403, or a transport failure.
        return "GITHUB_READ_TRANSIENT"
    # Without rate-limit headers, a 401 or 403 is the token's standing answer.
    return "GITHUB_READ_DENIED" if getattr(error, "status", None) in (401, 403) else "GITHUB_READ_FAILED"


def refusal_code(error):
    """The fixed public code of an allocation failure."""
    if not isinstance(error, Refusal):
        return "UNEXPECTED_ERROR"
    code = REFUSAL_CODES.get(str(error), "UNCLASSIFIED_REFUSAL")
    return github_read_code(error) if code == "GITHUB_READ_FAILED" else code


def run_allocation(operation, env=None):
    # Never echo exception text or private output. Only this closed vocabulary
    # crosses the existing output boundary; unknown failures remain generic.
    reason, site = "UNEXPECTED_ERROR", None

    def guarded(masker):
        nonlocal reason, site
        try:
            return operation(masker)
        except Exception as error:
            site = error_site(error)
            reason = refusal_code(error)
            raise

    status = run_private(guarded, None, subject="allocation", explain=False)
    if status and reason in TRANSIENT_CODES:
        # No matrix is emitted, so nothing downstream runs. The step succeeds
        # so that its settlement gate, the next step, reports the refusal and
        # fails the job: that is the stop catch-up re-runs.
        env = os.environ if env is None else env
        output(env, "transient", "true")
        output(env, "refused", reason)
        if site is not None:
            output(env, "site", site)
        return 0
    if status:
        report("allocation", reason, subject="allocation", site=site)
    return status


def require_settled(code, site):
    """Fail the job for the transient refusal the allocation step recorded."""
    report("allocation", code if code in TRANSIENT_CODES else "UNCLASSIFIED_REFUSAL", subject="allocation",
           site=site or None)
    return 1


def owner_attempt(api, reservation):
    path = f"actions/runs/{reservation['run_id']}"
    try:
        run = api.request(path + f"/attempts/{reservation['run_attempt']}", allow_not_found=True)
    except GitHubNotFound:
        try:
            api.request(path, allow_not_found=True)
        except GitHubNotFound:
            # ABANDONED-BURNED: retain this reservation in the watermark.
            # Allocation creates a new version; it never resumes this one.
            return None
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
    return run


def run_active(api, reservation):
    run = owner_attempt(api, reservation)
    return run is not None and run["status"] != "completed"


def web_writer_failed(api, reservation):
    # The protected publication can fail while unrelated jobs keep its run
    # active. Only a terminal publication failure releases the Web owner.
    run_id, attempt = reservation["run_id"], reservation["run_attempt"]
    result = api.request(f"actions/runs/{run_id}/attempts/{attempt}/jobs?per_page=100")
    jobs = result.get("jobs") if isinstance(result, dict) else None
    require(isinstance(jobs, list) and type(result.get("total_count")) is int and result["total_count"] == len(jobs)
            and all(isinstance(job, dict) and job.get("run_id") == run_id
                    and job.get("run_attempt") == attempt for job in jobs),
            "GitHub API response is invalid")
    names = {
        "Release web / Publish",
        "Publish protected Web / Protect Web rollout / Finalize protected Web / Publish",
    }
    writers = [job for job in jobs if job.get("name") in names]
    return len(writers) == 1 and writers[0].get("status") == "completed" and writers[0].get("conclusion") in {
        "failure", "cancelled", "timed_out",
    }


def allocate_queue(queue, api, reservations, run_id, attempt, *, read_proofs=dict, mark=lambda _component, _phase: None):
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
    changes = durable_baselines(sorted(queue["changes"], key=lambda change: history.position(change["source"])),
                                pending, finals, history)
    newest = {name: change["source"] for change in changes for name in change["deployables"]}
    owners, busy = {}, set()
    group_sources = [newest[DEPLOYABLES[key]] for key in GROUP if DEPLOYABLES[key] in newest]
    transition = queue.get("contract_transition")
    group_tip = max(group_sources, key=history.position, default=None) if transition else None
    inventory = pending + [tag["receipt"]["identity"] for tag in finals]
    # A successor claims its complete group atomically. Once its newest source
    # has a reservation or final, replaying the queue cannot take ownership again.
    new_group_source = group_tip is not None and not any(
        item["component"] in GROUP and item["source"] == group_tip for item in inventory)
    for item in pending:
        if item["component"] not in DEPLOYABLES or DEPLOYABLES[item["component"]] not in newest:
            continue
        if source_is_retired(item["component"], item["source"]):
            continue
        # Only duplicate work for the very same source needs an owner lookup.
        # A strictly older owner that has not started building yields to the
        # successor; one already building finishes unless a higher final lands.
        if item["source"] != newest[DEPLOYABLES[item["component"]]]:
            continue
        if new_group_source and item["component"] in GROUP and item["source"] != group_tip:
            # A sibling source moves only with an actual group publication.
            # Defer it to the atomic group plan without waiting for its writer.
            busy.add(item["component"])
            continue
        owner = (item["run_id"], item["run_attempt"])
        if owner == (run_id, attempt):
            continue
        candidates = [value for value in inventory if value["component"] == item["component"]]
        # Use the same durable fence as publication. Queue intent alone cannot
        # revoke a writer between its last check and registry copy.
        if item["source"] not in history.positions or any(
                (value["source"] in history.positions
                 and history.position(value["source"]) > history.position(item["source"]))
                or version_tuple(value["version"]) > version_tuple(item["version"]) for value in candidates):
            continue
        owners.setdefault(owner, []).append(item)

    final_names = {tag["name"] for tag in finals}
    current_event = queue.get("event_source") == queue["head"] == history.head

    def owner_state(items):
        item = items[0]
        mark(item["component"], "owner-read")
        run = owner_attempt(api, item)
        active = run is not None and run["status"] != "completed"
        web = next((item for item in items if item["component"] == "web"), None)
        failed = web if active and current_event and web is not None and web_writer_failed(api, web) else None
        return active, failed

    to_reconcile = []
    for items, (active, failed) in zip(owners.values(), parallel_read(owner_state, owners.values())):
        for item in items:
            if active and item != failed:
                busy.add(item["component"])
            elif DEPLOYABLES[item["component"]] + "/v" + item["version"] not in final_names:
                to_reconcile.append(item)
    if transition and not new_group_source and busy & GROUP:
        # A partial resume would make group completion replace a live publisher.
        # Leave every peer to that owner while independent components proceed.
        busy.update(GROUP)
    if to_reconcile:
        settled = parallel_read(reservations.final_after_owner, to_reconcile)
        finals.extend(tag for tag in settled if tag is not None)
    # A failed publication owes a release, even at a successor with zero new Web
    # changes. Build that successor with its current publication policy.
    if current_event and "web" not in busy:
        final_sources = {tag["target"] for tag in finals if tag["receipt"]["identity"]["component"] == "web"}
        if any(item["component"] == "web" and item["source"] not in final_sources for item in to_reconcile):
            changes = include_web_head(api, changes, history.head)
            newest[DEPLOYABLES["web"]] = history.head
    latest = {}
    for tag in sorted(finals, key=lambda tag: history.position(tag["target"])):
        receipt = tag["receipt"]
        latest[receipt["identity"]["component"]] = receipt
    # A deployment can finish while this run is waiting for its allocator or
    # reading Git inventory. Check outcomes only for finals this pass can resume.
    recoverable = any(component not in busy and newest.get(DEPLOYABLES[component]) == receipt["source"]
                      for component, receipt in latest.items())
    verified = proven_components(read_proofs(), latest, history) if recoverable else set()
    actions, needed_prs = [], {}
    for change in changes:
        source, tree, deployables = (change[key] for key in ("source", "tree", "deployables"))
        if not deployables:
            continue
        published = [tag["receipt"] for tag in finals if tag["target"] == source]
        for component, name in DEPLOYABLES.items():
            if name not in deployables or source != newest[name] or component in busy:
                continue
            # Keep the retired source in newest to prevent an implicit fallback
            # to its predecessor. An eligible successor may release normally.
            if source_is_retired(component, source):
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
                # Finalization proves which bytes exist. Only a matching
                # controller result proves that the latest writer applied them.
                receipt = existing[0]
                if (latest[component] == receipt and component not in verified
                        and (source == queue.get("event_source") or current_event)
                        and all(version_tuple(row["version"]) <= version_tuple(receipt["identity"]["version"])
                                for row in stored)):
                    jobs.append({"component": component, "mode": "resume", "receipt": existing[0]})
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
        jobs.append({"component": component, "mode": "publish", "reservation": value})
    # Queued sources without reservations are superseded. Existing versions
    # stay burned, and only the selected source supplies each new bump.
    complete = complete_group(transition, jobs, changes, api, reservations, finals, pending, run_id, attempt)
    grouped = [{"component": key, **({"contract_transition": transition} if complete and key in GROUP else {}),
                "tasks": canonical([job for job in jobs if job["component"] == key])}
               for key in DEPLOYABLES if any(job["component"] == key for job in jobs)]
    mark("all", "planned")
    return {"include": grouped}


def include_web_head(api, changes, head):
    rows = {row["source"]: row for row in changes}
    if head in rows:
        row = rows[head]
        rows[head] = dict(row, deployables=sorted({*row["deployables"], DEPLOYABLES["web"]}))
    else:
        commit = api.request("git/commits/" + head)
        tree = commit.get("tree", {}).get("sha") if isinstance(commit, dict) and isinstance(commit.get("tree"), dict) else None
        require(isinstance(commit, dict) and commit.get("sha") == head and matches(r"[0-9a-f]{40}", tree),
                "Release change identity is invalid")
        rows[head] = {"source": head, "tree": tree, "deployables": [DEPLOYABLES["web"]]}
    return list(rows.values())


def main(argv=()):
    env, argv = os.environ, list(argv)
    if argv == ["--require-settled"]:
        return require_settled(env.get("CD_ALLOCATION_REFUSED"), env.get("CD_ALLOCATION_SITE"))
    if argv:
        report("step", "refused", subject="allocation")
        return 1
    def operation(_masker):
        event_identity(env)
        require(env["GITHUB_EVENT_NAME"] == "push", "Version allocation only accepts develop pushes")
        queue = json.loads(env["CD_QUEUE"])
        require(isinstance(queue, dict) and matches(r"[0-9a-f]{40}", queue.get("event_source"))
                and queue["event_source"] == env["GITHUB_SHA"], "Release queue is invalid")
        matrix = allocate_attempts(queue, env, mark, mark.retry)
        mark("all", "complete")
        output(env, "matrix", canonical(matrix))
        output(env, "changed", "true" if matrix["include"] else "false")
        return True
    with allocation_progress() as mark:
        return run_allocation(operation, env)


def allocate_attempts(queue, env, mark, retry, *, sleep=time.sleep, rand=random.random):
    """Allocate, once more after a transient refusal raised before the push started."""
    for attempt in range(1, ALLOCATION_ATTEMPTS + 1):
        io = AllocationIO()
        try:
            matrix = allocate_current(queue, env, io, mark)
            io.remaining()
            return matrix
        except Refusal as error:
            code = refusal_code(error)
            requested = ci_retry.requested_wait(error) or 0
            # Once the push has started, the remote may hold this attempt's
            # refs: settlement and the gate decide, as for every other stop.
            if (attempt == ALLOCATION_ATTEMPTS or io.push_started or code not in RETRIED_CODES
                    or requested > RETRY_WAIT_SECONDS[1]):
                raise
        low, high = RETRY_WAIT_SECONDS
        wait = max(low + (high - low) * rand(), requested)
        retry(code, attempt + 1, ALLOCATION_ATTEMPTS, wait)
        sleep(wait)


def allocate_current(queue, env, io, mark):
    # A lost race proves another atomic allocation completed. Only then may we
    # plan a distinct transaction, from a fresh history and complete inventory.
    require(matches(r"[0-9a-f]{40}", queue.get("head")) and isinstance(queue.get("changes"), list),
            "Release queue is invalid")
    if queue["event_source"] != queue["head"]:
        mark("all", "stale-source")
        return {"include": []}
    def read_proofs():
        return available_proofs(env.get("GH_TOKEN"), env["CD_STAGING_ENVIRONMENT"])
    api = GitHub(env.get("GH_TOKEN"), io)
    while True:
        io.remaining()
        reservations = Reservations(api, env.get("GH_TOKEN"), Path.cwd(), io, mark=mark)
        if not reservations.load(event_source=queue["event_source"]):
            mark("all", "stale-source")
            return {"include": []}
        matrix = allocate_queue(queue, SnapshotAPI(api, reservations), reservations,
                                int(env["GITHUB_RUN_ID"]), int(env["GITHUB_RUN_ATTEMPT"]),
                                read_proofs=read_proofs, mark=mark)
        try:
            reservations.commit()
        except AllocationConflict:
            mark("all", "replan")
            continue
        return matrix


if __name__ == "__main__":
    raise SystemExit(main(sys.argv[1:]))
