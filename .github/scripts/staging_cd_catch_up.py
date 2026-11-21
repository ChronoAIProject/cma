"""Re-plan lanes a failed run left behind, once every staging run is idle.

Allocation skips a lane while another run owns its newest reservation. When
that owner's build then fails, nothing plans the lane again until the next
develop push. Likewise a lane whose newest final was published but never
proven on staging waits for the next push, whose allocation resumes it.
After each Staging CD run completes, and hourly as a backstop, this check
re-runs the develop head's own push run once when no Staging CD run is
active, that run's allocation completed, and some lane's newest version is
either an unfinalized reservation of an earlier run or a final of a settled
owner that no controller proof covers. A re-run for finals alone waits for
the hourly check, once its head has settled for a while, so a stuck app or
a namespace fence that failed the deploy can clear first, and never follows
a cancelled head. A re-run attempt
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
"""
import collections
from datetime import datetime
import json
import os
from pathlib import Path
import subprocess
import time

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
# The display name of staging-cd.yml's allocation job; a workflow test pins it.
ALLOCATION_JOB = "allocate"
# One automatic re-run per develop head; never re-run a head this many times.
ATTEMPTS = 5
SECONDS = 150
# A re-run for finals alone waits until its head has been settled this long.
SETTLE_SECONDS = 600
# Nothing waits on this check, so the controller read may repeat for longer than the planner's.
CONTROLLER_SECONDS = 30
TIMESTAMP = r"\d{4}-\d{2}-\d{2}T\d{2}:\d{2}:\d{2}Z"
# Why a lane is not current. Only the first three re-run its head.
BEHIND = ("unfinalized-reservation", "unproven-final", "undeployed-final")
UNKNOWN = "controller-unknown"
Lane = collections.namedtuple("Lane", "component reason version")


def push_run(run):
    return (isinstance(run, dict) and type(run.get("id")) is int and run["id"] > 0
            and type(run.get("run_attempt")) is int and run["run_attempt"] > 0
            and run.get("event") == "push" and run.get("head_branch") == "develop"
            and matches(r"[0-9a-f]{40}", run.get("head_sha"))
            and isinstance(run.get("repository"), dict) and run["repository"].get("full_name") == REPOSITORY)


def advertised(runner=subprocess.run):
    # The repository is public. One advertisement binds develop to every tag.
    env = {key: value for key, value in os.environ.items() if key in ("PATH", "LANG", "LC_ALL")}
    env.update(GIT_CONFIG_NOSYSTEM="1", GIT_CONFIG_GLOBAL=os.devnull, GIT_TERMINAL_PROMPT="0")
    result = runner(["git", "-c", "credential.helper=", "-c", "http.extraHeader=", "ls-remote", "--refs",
                     REMOTE, DEVELOP, "refs/tags/*"], env=env, text=True, capture_output=True,
                    timeout=60, check=False)
    require(result.returncode == 0 and len(result.stdout) <= 16 * 1024 * 1024, "Release tag inventory is incomplete")
    refs = {}
    for line in result.stdout.splitlines():
        oid, _, ref = line.partition("\t")
        require(matches(r"[0-9a-f]{40}", oid) and (ref == DEVELOP or ref.startswith("refs/tags/"))
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
    jobs = api.request(f"actions/runs/{run['id']}/attempts/{run['run_attempt']}/jobs?per_page=100")
    require(isinstance(jobs, dict) and isinstance(jobs.get("jobs"), list), "Head run jobs could not be read")
    return [job for job in jobs["jobs"] if isinstance(job, dict)]


def allocated(jobs):
    rows = [job for job in jobs if job.get("name") == ALLOCATION_JOB]
    return len(rows) == 1 and rows[0].get("status") == "completed" and rows[0].get("conclusion") == "success"


def settled(run, now):
    try:
        finished = datetime.fromisoformat(run["updated_at"].replace("Z", "+00:00")).timestamp()
    except (AttributeError, KeyError, TypeError, ValueError):
        return False
    return now - finished >= SETTLE_SECONDS


def decide(api, read_refs, read_controller, *, event=None, now=None):
    listing = api.request("actions/workflows/staging-cd.yml/runs?branch=develop&per_page=100")
    require(isinstance(listing, dict) and isinstance(listing.get("workflow_runs"), list)
            and all(isinstance(run, dict) for run in listing["workflow_runs"]), "Staging runs could not be read")
    # The last run to finish decides; a running one (or a pending re-run) will.
    if any(run.get("status") != "completed" for run in listing["workflow_runs"]):
        return "run-active", None, []
    refs = read_refs()
    runs = [run for run in listing["workflow_runs"] if run.get("head_sha") == refs[DEVELOP]]
    if not runs:
        return "head-run-missing", None, []
    require(len(runs) == 1 and push_run(runs[0]) and runs[0].get("path") == WORKFLOW,
            "Develop head run identity is invalid")
    run = runs[0]
    actor = run.get("triggering_actor")
    # The bot's attempt was this head's automatic re-run; a maintainer's earns one more.
    spent = (isinstance(actor, dict) and actor.get("login") == BOT) or run["run_attempt"] >= ATTEMPTS
    jobs = run_jobs(api, run)
    # A failed plan or allocation is a failure to retry on the next push, not a skip.
    if not spent and not allocated(jobs):
        return "head-unallocated", None, []
    wait, lanes = left_behind(api, refs, run, jobs, read_controller)
    if wait:
        return wait, None, []
    late = [lane for lane in lanes if lane.reason in BEHIND]
    if late and spent:
        return "exhausted", run, lanes
    if late and all(lane.reason != BEHIND[0] for lane in late):
        # A person or a job timeout cancelled the head; a build left behind still re-runs.
        if run.get("conclusion") == "cancelled":
            return "cancelled", None, lanes
        # A deploy failure's cause may outlast its run; the single retry waits for it.
        current = time.time() if now is None else now
        if event != "schedule" or not settled(run, current):
            return "final-waiting", None, lanes
    if late:
        return "rerun", run, lanes
    if spent:
        return "head-caught-up", None, lanes
    return (UNKNOWN if lanes else "nothing-behind"), None, lanes


def rerun(api, run, *, sleep=time.sleep):
    path = f"actions/runs/{run['id']}"
    current = api.request(path)
    require(push_run(current) and current["id"] == run["id"] and current["run_attempt"] == run["run_attempt"]
            and current.get("status") == "completed", "Develop head run changed before its catch-up")
    try:
        api.request(path + "/rerun", {"enable_debug_logging": False})
    except Refusal:
        pass  # GitHub answers 201 without a body; a lost answer is settled by reading back.
    for attempt in range(3):
        after = api.request(path)
        require(push_run(after) and after["id"] == run["id"], "Develop head run identity is invalid")
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
            reason, run, lanes = decide(api, advertised, controller_reader(env), event=env["GITHUB_EVENT_NAME"])
            outcome.update(reason=reason, lanes=lanes)
            if reason == "rerun":
                outcome.update(run=run["id"], attempt=rerun(api, run))
            elif reason == "exhausted":
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
    elif reason == "exhausted":
        verb, pronoun = ("stays", "it") if len(late) == 1 else ("stay", "them")
        print(f"::warning title=Staging CD catch-up exhausted::{','.join(late)} {verb} behind after attempt "
              f"{outcome['attempt']} of run {outcome['run']}; a push or a maintainer re-run retries {pronoun}")
        for component in late:
            print(f"CATCH-UP exhausted lane={component} attempts={outcome['attempt']}")
    elif reason == "cancelled":
        print("CATCH-UP stop reason=cancelled")
    else:
        print("CATCH-UP " + reason)
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
