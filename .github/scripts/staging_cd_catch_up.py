"""Re-plan lanes a busy owner left behind, once every staging run is idle.

Allocation skips a lane while another run owns its newest reservation. When
that owner's build then fails, nothing plans the lane again until the next
develop push. After each Staging CD run completes, and hourly as a backstop,
this check re-runs the develop head's own push run once when no Staging CD run
is active, that run's allocation completed, and some lane's newest version is
an unfinalized reservation of an earlier run. A re-run attempt is the same
develop push event, so every push-only authority, cache and admission check
applies unchanged and staging-cd.yml gains no trigger.
"""
import json
import os
from pathlib import Path
import subprocess
import time

from staging_cd_common import Refusal, matches, require
from staging_cd_github import GitHub, GitHubNotFound
from staging_cd_receipts import parse_annotation
from staging_cd_reservations import pending_name, validate_reservation
from staging_cd_runtime import run_private
from staging_cd_versions import DEPLOYABLES, REPOSITORY, version_tuple

WORKFLOW = ".github/workflows/staging-cd.yml"
CHECK = ".github/workflows/staging-cd-catch-up.yml"
DEVELOP = "refs/heads/develop"
REMOTE = "https://github.com/ChronoAIProject/cma.git"
BOT = "github-actions[bot]"
# One automatic re-run per develop head; never re-run a head this many times.
ATTEMPTS = 5
SECONDS = 150


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


def final_source(api, component, version, oid):
    name = DEPLOYABLES[component] + "/v" + version
    tag = tag_object(api, name, oid)
    try:
        receipt = parse_annotation(tag.get("message"))
    except (TypeError, ValueError) as error:
        raise Refusal("Tag receipt differs from the annotated commit") from error
    require(receipt["tag"] == name and receipt["source"] == tag["object"]["sha"]
            and receipt["identity"]["component"] == component, "Tag receipt differs from the annotated commit")
    return receipt["source"]


def owner_settled(api, value):
    try:
        run = api.request(f"actions/runs/{value['run_id']}/attempts/{value['run_attempt']}", allow_not_found=True)
    except GitHubNotFound:
        return True  # ABANDONED-BURNED: a deleted owner can never finish its build.
    require(push_run(run) and run["id"] == value["run_id"] and run["run_attempt"] == value["run_attempt"],
            "Reservation run identity is invalid")
    return run.get("status") == "completed"


def left_behind(api, refs, head_run):
    """Lanes whose newest version is an unfinalized reservation of an earlier run.

    Versions only grow, so a lane's newest version is its latest attempt. A
    finalized one is current. One the head's run made itself failed at the
    newest opportunity and waits for the next push, as any failure does. A
    same-source group claim that yielded to its finalized peer is current.
    Every other unfinalized newest reservation predates the head's allocation,
    which would have retried it unless its owner was still running then.
    """
    lanes = []
    for component, (finals, pending) in sorted(lane_versions(refs).items()):
        versions = finals.keys() | pending.keys()
        if not versions:
            continue
        newest = max(versions, key=version_tuple)
        if newest in finals:
            continue
        value = reservation(api, component, newest, pending[newest])
        if value["run_id"] == head_run["id"]:
            continue
        if finals:
            latest = max(finals, key=version_tuple)
            if final_source(api, component, latest, finals[latest]) == value["source"]:
                continue
        if not owner_settled(api, value):
            return None  # Its completion triggers the next check.
        lanes.append(component)
    return lanes


def allocated(api, run):
    jobs = api.request(f"actions/runs/{run['id']}/attempts/{run['run_attempt']}/jobs?per_page=100")
    require(isinstance(jobs, dict) and isinstance(jobs.get("jobs"), list), "Head run jobs could not be read")
    rows = [job for job in jobs["jobs"] if isinstance(job, dict) and job.get("name") == "allocate"]
    return len(rows) == 1 and rows[0].get("status") == "completed" and rows[0].get("conclusion") == "success"


def decide(api, read_refs):
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
    if (isinstance(actor, dict) and actor.get("login") == BOT) or run["run_attempt"] >= ATTEMPTS:
        return "head-caught-up", None, []
    # A failed plan or allocation is a failure to retry on the next push, not a skip.
    if not allocated(api, run):
        return "head-unallocated", None, []
    lanes = left_behind(api, refs, run)
    if lanes is None:
        return "owner-active", None, []
    if not lanes:
        return "nothing-behind", None, []
    return "rerun", run, lanes


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
            and env.get("GITHUB_EVENT_NAME") in ("workflow_run", "schedule"),
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


def main():
    env, outcome = os.environ, {}

    def operation(_masker):
        try:
            authorized(env)
            api = GitHub(env.get("GH_TOKEN"), deadline=time.monotonic() + SECONDS)
            reason, run, lanes = decide(api, advertised)
            outcome.update(reason=reason, lanes=lanes)
            if run is not None:
                outcome.update(run=run["id"], attempt=rerun(api, run))
        except Refusal as error:
            outcome["failure"] = str(error)  # Refusals carry only fixed messages.
            raise
        return True

    status = run_private(operation, None)
    if status:
        print("FAIL catch-up " + outcome.get("failure", "unexpected error"))
        return status
    line = "CATCH-UP " + outcome["reason"]
    if outcome["lanes"]:
        line += f" run={outcome['run']} attempt={outcome['attempt']} lanes={','.join(outcome['lanes'])}"
    print(line)
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
