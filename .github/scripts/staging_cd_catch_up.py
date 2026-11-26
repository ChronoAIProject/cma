"""Re-plan lanes a failed run left behind, once every staging run is idle.

Allocation skips a lane while another run owns its newest reservation. When
that owner's build then fails, nothing plans the lane again until the next
develop push. Likewise a lane whose newest final was published but never
proven on staging waits for the next push, whose allocation resumes it.
This check runs after each Staging CD run completes and every ten minutes.
It re-runs the develop head's own push run once when no Staging CD run is
active, that run's allocation completed, and some lane's newest version is
either an unfinalized reservation of an earlier run or a final of a settled
owner that no controller proof covers. A re-run for finals alone waits until
every staging run has settled for a while so a stuck app or a namespace fence
that failed the deploy can clear first, and never follows a cancelled head.
A completion check that needs to wait schedules a fresh check outside the
shared catch-up lock. A re-run attempt
is the same develop push event, so every push-only authority, cache and
admission check applies unchanged, its admission clock starts afresh, and
staging-cd.yml gains no trigger. Like any allocation it also retries the
head's own failed builds. Those alone never trigger a re-run, but finals
they leave undeployed do: a group transition holds its runtimes' finals
for a failed AM build.

The re-run is automatic once per head: its attempt is triggered by the bot,
which never earns another, and no head runs ATTEMPTS times. A lane still
behind after that is reported as exhausted until a push or a maintainer acts.
An unreadable controller outcome is reported as unknown and never re-runs.

A head run whose plan stopped at its completion gate (a budget-stopped plan,
or tags that could not be read), or whose allocation stopped at its
settlement gate (a transient read, write or budget refusal), is re-run once
too, with the same settling: once every staging run has settled, never after
a cancelled head, and never by the bot's own
attempt. Reservations a stopped allocation committed belong to its settled
attempt, so the re-run attempt reserves their sources again. A spent head
whose plan or allocation stopped at its gate again is never caught up: it is
reported as exhausted, with a warning, until a push or a maintainer acts.
"""
import collections
from datetime import datetime
import json
import os
from pathlib import Path
import subprocess
import time

import ci_retry
from staging_cd_common import Refusal, matches, require
from staging_cd_github import GitHub, GitHubNotFound
from staging_cd_promotion import UNAVAILABLE, job_proof, proven_components, push_scope, read_snapshot
from staging_cd_receipts import parse_annotation
from staging_cd_reservations import pending_name, validate_reservation
from staging_cd_runtime import run_private
from staging_cd_targets import TARGETS
from staging_cd_versions import DEPLOYABLES, REPOSITORY, version_tuple

WORKFLOW = ".github/workflows/staging-cd.yml"
CHECK = ".github/workflows/staging-cd-catch-up.yml"
DEVELOP = "refs/heads/develop"
REMOTE = "https://github.com/ChronoAIProject/cma.git"
BOT = "github-actions[bot]"
# The display names of staging-cd.yml's allocation job, its plan job and each
# job's gate for a transient stop; a workflow test pins them.
ALLOCATION_JOB = "allocate"
ALLOCATION_GATE = "Require a settled allocation"
PLAN_JOB = "plan"
PLAN_GATE = "Require a complete release plan"
# Only these decisions re-run the head run; any other names no action.
RERUNS = frozenset({"rerun", "plan-rerun", "allocation-rerun"})
# A spent head still behind, or stopped at a gate again, is reported with a warning.
EXHAUSTED = frozenset({"exhausted", "plan-exhausted", "allocation-exhausted"})
# One automatic re-run per develop head; never re-run a head this many times.
ATTEMPTS = 5
SECONDS = 150
# A re-run for finals alone waits until every staging run has been settled this long.
SETTLE_SECONDS = 600
# Nothing waits on this check, so the controller read may repeat for longer than the planner's.
CONTROLLER_SECONDS = 30
TIMESTAMP = r"\d{4}-\d{2}-\d{2}T\d{2}:\d{2}:\d{2}Z"
# Why a lane is not current. Only the first three re-run its head.
BEHIND = ("unfinalized-reservation", "unproven-final", "undeployed-final")
UNKNOWN = "controller-unknown"
Lane = collections.namedtuple("Lane", "component reason version")
Decision = collections.namedtuple("Decision", "reason run lanes settle_until", defaults=(None,))


def push_run(run):
    return (isinstance(run, dict) and type(run.get("id")) is int and run["id"] > 0
            and type(run.get("run_attempt")) is int and run["run_attempt"] > 0
            and run.get("event") == "push" and run.get("head_branch") == "develop"
            and matches(r"[0-9a-f]{40}", run.get("head_sha"))
            and isinstance(run.get("repository"), dict) and run["repository"].get("full_name") == REPOSITORY)


def advertised(runner=subprocess.run, *, include_tags=True):
    # One advertisement binds develop to every tag. The write fence only needs develop.
    patterns = [DEVELOP, "refs/tags/*"] if include_tags else [DEVELOP]
    env = {key: value for key, value in os.environ.items() if key in ("PATH", "LANG", "LC_ALL")}
    env.update(GIT_CONFIG_NOSYSTEM="1", GIT_CONFIG_GLOBAL=os.devnull, GIT_TERMINAL_PROMPT="0")
    result = runner(["git", "-c", "credential.helper=", "-c", "http.extraHeader=", "ls-remote", "--refs",
                     REMOTE, *patterns], env=env, text=True, capture_output=True,
                    timeout=60, check=False)
    if result.returncode != 0:
        raise Refusal("Release ref inventory is incomplete") from ci_retry.cli_cause(result.stderr)
    require(len(result.stdout) <= 16 * 1024 * 1024, "Release ref inventory is incomplete")
    refs = {}
    for line in result.stdout.splitlines():
        oid, _, ref = line.partition("\t")
        require(matches(r"[0-9a-f]{40}", oid) and (ref == DEVELOP or (include_tags and ref.startswith("refs/tags/")))
                and ref not in refs, "Release tag inventory is malformed")
        refs[ref] = oid
    require(DEVELOP in refs, "Develop head could not be verified")
    return refs


def lane_versions(refs):
    lanes = {}
    for component, name in DEPLOYABLES.items():
        finals, pending = {}, {}
        for ref, oid in refs.items():
            tag = ref.removeprefix("refs/tags/")
            for prefix, versions in ((name + "/pending/v", pending), (name + "/v", finals)):
                if tag.startswith(prefix):
                    version = tag[len(prefix):]
                    version_tuple(version)
                    versions[version] = oid
        lanes[component] = finals, pending
    return lanes


def tag_object(api, name, oid):
    tag = api.request("git/tags/" + oid)
    require(isinstance(tag, dict) and tag.get("sha") == oid and tag.get("tag") == name
            and isinstance(tag.get("object"), dict) and tag["object"].get("type") == "commit"
            and matches(r"[0-9a-f]{40}", tag["object"].get("sha")), "Release tag identity differs")
    return tag


def reservation(api, component, version, oid):
    name = DEPLOYABLES[component] + "/pending/v" + version
    tag = tag_object(api, name, oid)
    try:
        value = validate_reservation(json.loads(tag.get("message", "")))
    except (TypeError, ValueError) as error:
        raise Refusal("Pending version ref differs from its reservation") from error
    require(value["component"] == component and pending_name(value) == name
            and value["source"] == tag["object"]["sha"], "Pending version ref differs from its reservation")
    return value


def final_receipt(api, component, version, oid):
    name = DEPLOYABLES[component] + "/v" + version
    tag = tag_object(api, name, oid)
    try:
        receipt = parse_annotation(tag.get("message"))
    except (TypeError, ValueError) as error:
        raise Refusal("Tag receipt differs from the annotated commit") from error
    require(receipt["tag"] == name and receipt["source"] == tag["object"]["sha"]
            and receipt["identity"]["component"] == component, "Tag receipt differs from the annotated commit")
    return receipt


def final_source(api, component, version, oid):
    # Kept for callers that need only the verified source of a final.
    return final_receipt(api, component, version, oid)["source"]


def owner_attempt(api, value):
    try:
        run = api.request(f"actions/runs/{value['run_id']}/attempts/{value['run_attempt']}", allow_not_found=True)
    except GitHubNotFound:
        return None  # ABANDONED-BURNED: a deleted owner can never finish its build.
    require(push_run(run) and run["id"] == value["run_id"] and run["run_attempt"] == value["run_attempt"],
            "Reservation run identity is invalid")
    return run


def owner_settled(run):
    return run is None or run.get("status") == "completed"


def left_behind(api, refs, head_run, jobs, read_controller):
    """Each lane that is not current and why, or the reason to wait instead.

    Versions only grow, so a lane's newest version is its latest attempt. One
    the head's run reserved itself failed at the newest opportunity; it never
    triggers a re-run by itself, so it waits for the next push or is retried by
    a re-run another lane triggered. Finals it leaves undeployed, such as the
    runtimes a group transition holds for it, are judged as finals below. A
    same-source group claim that yielded to its finalized peer is current.
    Every other unfinalized newest reservation predates the head's allocation,
    which would have retried it unless its owner was still running then.

    A newest final is current once a controller proof covers it, whether from
    the newest staging writers or the head's own deploy. Otherwise it is
    behind when its lane's newest writer left no proof of it, or when no writer
    has covered the lane since the final's owner started. A lane whose last
    writer is older than the recent listing waits for the next push. Only a
    staging target's final can be proven, so no other lane's final is judged.
    """
    lanes, finals_due, latest = [], {}, {}
    for component, (finals, pending) in sorted(lane_versions(refs).items()):
        versions = finals.keys() | pending.keys()
        if not versions:
            continue
        newest = max(versions, key=version_tuple)
        if finals:
            top = max(finals, key=version_tuple)
            latest[component] = final_receipt(api, component, top, finals[top])
        if newest in finals:
            if component in TARGETS:
                finals_due[component] = newest
            continue
        value = reservation(api, component, newest, pending[newest])
        if value["run_id"] == head_run["id"]:
            continue
        if finals and latest[component]["source"] == value["source"]:
            continue
        if not owner_settled(owner_attempt(api, value)):
            return "owner-active", []  # Its completion triggers the next check.
        lanes.append(Lane(component, BEHIND[0], newest))
    if not finals_due:
        return None, lanes
    try:
        controller = read_controller()
        history = None if controller is None else api.history_at(refs[DEVELOP], root=Path.cwd())
    except UNAVAILABLE:
        controller = None
    if controller is None:
        # Without a complete outcome a final is neither current nor behind.
        return None, lanes + [Lane(key, UNKNOWN, version) for key, version in sorted(finals_due.items())]
    proven = proven_components(controller.proofs, latest, history)
    for evidence in head_proofs(head_run, jobs):
        proven |= proven_components(evidence, latest, history)
    for component, version in sorted(finals_due.items()):
        if component in proven:
            continue
        if component in controller.running:
            return "deploy-active", []  # Its writer's run completes or the schedule checks again.
        owner = owner_attempt(api, latest[component]["identity"])
        if not owner_settled(owner):
            return "owner-active", []
        if component in controller.observed:
            lanes.append(Lane(component, BEHIND[1], version))
        elif undeployed(owner, controller.since):
            lanes.append(Lane(component, BEHIND[2], version))
    return None, lanes


def head_proofs(head_run, jobs):
    """The controller proofs the head attempt's own deploy jobs left."""
    for job in jobs:
        scope = push_scope(job.get("name"))
        value = None if scope is None else job_proof(job, scope, head_run["head_sha"])
        if value:
            yield {key: value for key in scope}


def undeployed(owner, since):
    # Every deployment created after the oldest listed one is listed, so a lane
    # none of them covered has had no writer since its final's owner started.
    if since is None:
        return True  # The listing holds every staging deployment.
    started = owner.get("run_started_at") if owner is not None else None
    return matches(TIMESTAMP, since) and matches(TIMESTAMP, started) and started > since


def run_jobs(api, run):
    listing = api.request(f"actions/runs/{run['id']}/attempts/{run['run_attempt']}/jobs?per_page=100")
    require(isinstance(listing, dict) and isinstance(listing.get("jobs"), list)
            and type(listing.get("total_count")) is int and listing["total_count"] == len(listing["jobs"]),
            "Head run jobs could not be read")
    jobs = listing["jobs"]
    require(all(isinstance(job, dict) and type(job.get("id")) is int and job["id"] > 0
                and type(job.get("run_id")) is int and job["run_id"] == run["id"]
                and type(job.get("run_attempt")) is int and job["run_attempt"] == run["run_attempt"]
                and job.get("head_sha") == run["head_sha"] for job in jobs)
            and len({job["id"] for job in jobs}) == len(jobs), "Head run job identity differs")
    return jobs


def allocated(jobs):
    rows = [job for job in jobs if job.get("name") == ALLOCATION_JOB]
    return len(rows) == 1 and rows[0].get("status") == "completed" and rows[0].get("conclusion") == "success"


def stopped_at(jobs, name, gate):
    """The job failed at its gate for a transient stop, not at a refusal or an error."""
    rows = [job for job in jobs if job.get("name") == name]
    if (len(rows) != 1 or rows[0].get("status") != "completed" or rows[0].get("conclusion") != "failure"
            or not isinstance(rows[0].get("steps"), list)):
        return False
    failed = [step.get("name") for step in rows[0]["steps"] if isinstance(step, dict)
              and step.get("conclusion") == "failure"]
    return failed == [gate]


def plan_stopped(jobs):
    return stopped_at(jobs, PLAN_JOB, PLAN_GATE)


def allocation_stopped(jobs):
    return stopped_at(jobs, ALLOCATION_JOB, ALLOCATION_GATE)


def settle_deadline(runs, now):
    """When the newest completion settles, or None without a trustworthy clock."""
    finished = [run.get("updated_at") for run in runs]
    if not all(matches(TIMESTAMP, value) for value in finished):
        return None
    try:
        latest = max(datetime.fromisoformat(value.replace("Z", "+00:00")).timestamp() for value in finished)
    except (OSError, OverflowError, ValueError):
        return None
    deadline = int(latest) + SETTLE_SECONDS
    return deadline if latest <= now else None


def decide(api, read_refs, read_controller, *, now=None):
    listing = api.request("actions/workflows/staging-cd.yml/runs?branch=develop&per_page=100")
    require(isinstance(listing, dict) and isinstance(listing.get("workflow_runs"), list)
            and all(isinstance(run, dict) for run in listing["workflow_runs"]), "Staging runs could not be read")
    staging_runs = listing["workflow_runs"]
    # The last run to finish decides; a running one (or a pending re-run) will.
    if any(run.get("status") != "completed" for run in staging_runs):
        return Decision("run-active", None, [])
    refs = read_refs()
    runs = [run for run in staging_runs if run.get("head_sha") == refs[DEVELOP]]
    if not runs:
        return Decision("head-run-missing", None, [])
    require(len(runs) == 1 and push_run(runs[0]) and runs[0].get("path") == WORKFLOW,
            "Develop head run identity is invalid")
    run = runs[0]
    actor = run.get("triggering_actor")
    # The bot's attempt was this head's automatic re-run; a maintainer's earns one more.
    spent = (isinstance(actor, dict) and actor.get("login") == BOT) or run["run_attempt"] >= ATTEMPTS
    jobs = run_jobs(api, run)
    # The stage whose transient gate stopped the head before it allocated, if any.
    stage = (None if allocated(jobs) else "plan" if plan_stopped(jobs)
             else "allocation" if allocation_stopped(jobs) else None)
    if not spent and not allocated(jobs):
        # A plan or an allocation stopped at its transient gate is re-run once,
        # as a re-run for finals alone is: never after a cancelled head, and
        # only once every staging run has settled. A failure outside its gate
        # waits for the next push.
        if stage is None:
            return Decision("head-unallocated", None, [])
        if run.get("conclusion") == "cancelled":
            return Decision("cancelled", None, [])
        current = time.time() if now is None else now
        deadline = settle_deadline(staging_runs, current)
        if deadline is None or deadline > current:
            return Decision(stage + "-waiting", None, [], deadline)
        return Decision(stage + "-rerun", run, [])
    wait, lanes = left_behind(api, refs, run, jobs, read_controller)
    if wait:
        return Decision(wait, None, [])
    late = [lane for lane in lanes if lane.reason in BEHIND]
    if spent and stage:
        # Its last attempt stopped at the gate again, so the head's own sources
        # were never allocated: whatever its lanes show, it is not caught up.
        return Decision(stage + "-exhausted", run, lanes)
    if late and spent:
        return Decision("exhausted", run, lanes)
    if late and all(lane.reason != BEHIND[0] for lane in late):
        # A person or a job timeout cancelled the head; a build left behind still re-runs.
        if run.get("conclusion") == "cancelled":
            return Decision("cancelled", None, lanes)
        # A deploy failure's cause may outlast its run; the single retry waits for it.
        current = time.time() if now is None else now
        deadline = settle_deadline(staging_runs, current)
        if deadline is None or deadline > current:
            return Decision("final-waiting", None, lanes, deadline)
    if late:
        return Decision("rerun", run, lanes)
    if spent:
        return Decision("head-caught-up", None, lanes)
    return Decision(UNKNOWN if lanes else "nothing-behind", None, lanes)


def rerun(api, run, read_head, *, sleep=time.sleep):
    path = f"actions/runs/{run['id']}"

    def same_push(row):
        return (push_run(row) and row["id"] == run["id"] and row["head_sha"] == run["head_sha"]
                and row.get("path") == WORKFLOW)

    current = api.request(path)
    require(same_push(current) and current["run_attempt"] == run["run_attempt"]
            and current.get("status") == "completed", "Develop head run changed before its catch-up")
    if read_head() != run["head_sha"]:
        return None
    try:
        api.request(path + "/rerun", {"enable_debug_logging": False})
    except Refusal:
        pass  # GitHub answers 201 without a body; a lost answer is settled by reading back.
    for attempt in range(3):
        after = api.request(path)
        require(same_push(after), "Develop head run identity is invalid")
        if after["run_attempt"] > run["run_attempt"]:
            return after["run_attempt"]
        if attempt < 2:
            sleep(2)
    raise Refusal("Catch-up re-run was not verified")


def authorized(env):
    require(env.get("GITHUB_REPOSITORY") == REPOSITORY and env.get("GITHUB_REF") == DEVELOP
            and env.get("GITHUB_WORKFLOW_REF") == f"{REPOSITORY}/{CHECK}@{DEVELOP}"
            and env.get("GITHUB_EVENT_NAME") in ("workflow_run", "schedule")
            and matches(r"[a-z0-9-]+", env.get("CD_STAGING_ENVIRONMENT")),
            "Catch-up requires its develop workflow")
    if env["GITHUB_EVENT_NAME"] == "workflow_run":
        event = json.loads(Path(env["GITHUB_EVENT_PATH"]).read_text())
        require(isinstance(event, dict), "Catch-up trigger is not a completed develop staging run")
        run = event.get("workflow_run")
        require(event.get("action") == "completed" and isinstance(run, dict) and run.get("path") == WORKFLOW
                and run.get("event") == "push" and run.get("head_branch") == "develop"
                and isinstance(run.get("head_repository"), dict)
                and run["head_repository"].get("full_name") == REPOSITORY,
                "Catch-up trigger is not a completed develop staging run")


def controller_reader(env):
    return lambda: read_snapshot(env.get("GH_TOKEN"), env.get("CD_STAGING_ENVIRONMENT"),
                                 seconds=CONTROLLER_SECONDS)


def main():
    env, outcome = os.environ, {}

    def operation(_masker):
        try:
            authorized(env)
            api = GitHub(env.get("GH_TOKEN"), deadline=time.monotonic() + SECONDS)
            reason, run, lanes, settle_until = decide(api, advertised, controller_reader(env))
            outcome.update(reason=reason, lanes=lanes)
            if settle_until is not None and env.get("GITHUB_OUTPUT"):
                with Path(env["GITHUB_OUTPUT"]).open("a") as output:
                    print(f"settle_until={settle_until}", file=output)
            if reason in RERUNS:
                attempt = rerun(api, run, lambda: advertised(include_tags=False)[DEVELOP])
                if attempt is None:
                    outcome.update(reason="head-moved", lanes=[])
                else:
                    outcome.update(run=run["id"], attempt=attempt)
            elif reason in EXHAUSTED:
                outcome.update(run=run["id"], attempt=run["run_attempt"])
        except Refusal as error:
            outcome["failure"] = str(error)  # Refusals carry only fixed messages.
            raise
        return True

    # This entry reports its own fixed refusal message below.
    status = run_private(operation, None, subject="catch-up", explain=False)
    if status:
        print("FAIL catch-up " + outcome.get("failure", "unexpected error"))
        return status
    reason, lanes = outcome["reason"], outcome["lanes"]
    late = [lane.component for lane in lanes if lane.reason in BEHIND]
    for lane in lanes:
        print(f"CATCH-UP lane={lane.component} reason={lane.reason} version={lane.version}")
    if reason == "rerun":
        print(f"CATCH-UP rerun run={outcome['run']} attempt={outcome['attempt']} lanes={','.join(late)}")
    elif reason in ("plan-rerun", "allocation-rerun"):
        print(f"CATCH-UP {reason} run={outcome['run']} attempt={outcome['attempt']}")
    elif reason == "exhausted":
        verb, pronoun = ("stays", "it") if len(late) == 1 else ("stay", "them")
        print(f"::warning title=Staging CD catch-up exhausted::{','.join(late)} {verb} behind after attempt "
              f"{outcome['attempt']} of run {outcome['run']}; a push or a maintainer re-run retries {pronoun}")
        for component in late:
            print(f"CATCH-UP exhausted lane={component} attempts={outcome['attempt']}")
    elif reason in EXHAUSTED:
        stage = reason.removesuffix("-exhausted")
        print(f"::warning title=Staging CD catch-up exhausted::the {stage} of run {outcome['run']} stopped at its "
              f"transient gate again after attempt {outcome['attempt']}; a push or a maintainer re-run retries it")
        print(f"CATCH-UP {reason} run={outcome['run']} attempts={outcome['attempt']}")
        for component in late:
            print(f"CATCH-UP exhausted lane={component} attempts={outcome['attempt']}")
    elif reason == "cancelled":
        print("CATCH-UP stop reason=cancelled")
    else:
        print("CATCH-UP " + reason)
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
