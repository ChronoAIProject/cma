"""Choose the staging-verified images for a production release; production never rebuilds.

A release reaches main as a merge commit M whose second parent R is the
develop commit the release PR qualified, with M's tree equal to R's. For
each component that production's private config enables, the release uses
the newest published version whose source is R or an ancestor of R on
develop's first-parent history. That exact image must have been deployed and
controller-verified by staging: a staging deployment evidence artifact,
written by staging-cd.yml or redeploy-staging.yml on develop, must record it
as controller-verified. The set is then rebuilt and re-verified by the same
dispatch_set path staging redeploys use.

Once pointer publication is activated, production can also select the Agent
runtime with exact pointer-mode staging evidence and the initial two-lane
qualification. The Machine Included runtime is restricted to staging.
"""

import hashlib
import heapq
import io
import json
import os
import subprocess
import sys
import tempfile
import time
import zipfile
from pathlib import Path

from staging_cd_common import Refusal, matches, require
from staging_cd_deploy_result import pointer_verified
from staging_cd_history import DevelopHistory

EVIDENCE_PREFIX = "staging-deployment-"
EVIDENCE_WORKFLOWS = (".github/workflows/staging-cd.yml", ".github/workflows/redeploy-staging.yml")
EVIDENCE_SCHEMA = "cma-component-deployment-v1"
VERIFIED = "controller-verified"
# Evidence is found through the staging deployment workflows' own runs, not
# the repository-wide artifact list: CI uploads hundreds of artifacts a day,
# so ten pages of that list covered about two hours, and a component staging
# had verified earlier could never be released. Runs are listed newest first.
EVIDENCE_RUN_PAGES = 5
MAX_RUN_LISTINGS = 300
# Each staging run keeps one evidence archive per lane, so a component last
# verified a few hours ago sits behind several archives per newer run.
MAX_DOWNLOADS = 200
DOWNLOAD_ATTEMPTS = 3
# staging-promote.yml names each lane's evidence after the lane; a lane that
# is not exactly one component (contract) or no suffix is always read.
COMPONENT_LANES = frozenset({"am", "eg", "web", "runtime", "runtime-omarchy"})
MAX_EVIDENCE_BYTES = 65536
MAIN = "refs/heads/main"


def git(root, *args, runner=subprocess.run):
    env = {k: v for k, v in os.environ.items() if k in ("PATH", "LANG", "LC_ALL")}
    env.update(GIT_CONFIG_NOSYSTEM="1", GIT_CONFIG_GLOBAL=os.devnull, GIT_TERMINAL_PROMPT="0")
    result = runner(["git", "-C", str(root), *args], env=env, capture_output=True, text=True,
                    timeout=60, check=False)
    require(result.returncode == 0, "Release commit ancestry could not be read")
    return result.stdout.strip()


def release_commit(merge, root, *, runner=subprocess.run):
    """Return R for main merge M: M has exactly two parents and the tree of its second."""
    require(matches(r"[0-9a-f]{40}", merge), "Production release commit is invalid")
    parents = git(root, "rev-list", "--parents", "-n", "1", merge, runner=runner).split()
    require(len(parents) == 3 and parents[0] == merge,
            "Production releases must reach main as a merge commit of the qualified develop commit")
    released = parents[2]
    require(git(root, "rev-parse", merge + "^{tree}", runner=runner)
            == git(root, "rev-parse", released + "^{tree}", runner=runner),
            "Main differs from the qualified develop commit it merged")
    return released


def main_head(*, runner=subprocess.run):
    """Main's current head from one credential-free public ref advertisement."""
    env = {k: v for k, v in os.environ.items() if k in ("PATH", "LANG", "LC_ALL")}
    env.update(GIT_CONFIG_NOSYSTEM="1", GIT_CONFIG_GLOBAL=os.devnull, GIT_TERMINAL_PROMPT="0")
    try:
        result = runner(["git", "-c", "credential.helper=", "ls-remote", "--refs",
                         "https://github.com/ChronoAIProject/cma.git", MAIN],
                        env=env, capture_output=True, text=True, timeout=30, check=False)
    except (OSError, subprocess.SubprocessError) as error:
        raise Refusal("Main head could not be verified") from error
    heads = [line.partition("\t")[0] for line in result.stdout.splitlines() if line.partition("\t")[2] == MAIN]
    require(result.returncode == 0 and len(heads) == 1 and matches(r"[0-9a-f]{40}", heads[0]),
            "Main head could not be verified")
    return heads[0]


def release_bounds(env, root, *, head=main_head, runner=subprocess.run):
    """Return this run's release commit R and develop's first parents from main's newest release.

    Main is the release of record, so production converges to its newest
    release. A higher final supersedes a receipt only when its source is on
    that release's develop history; a later develop final belongs to a later
    release's run. Measuring from the newest release, not from R, means a
    deployment for an older merge that was approved late or re-run hands
    over to the newer release's run instead of rolling production back.
    """
    released = release_commit(env["GITHUB_SHA"], root, runner=runner)
    newest = head()
    # The fetch makes main's head readable; this merge must be on its first parents.
    require(env["GITHUB_SHA"] in DevelopHistory.from_head(newest, root=root).positions,
            "Production release is not on main's first-parent history")
    history = DevelopHistory.from_head(release_commit(newest, root, runner=runner), root=root)
    require(released in history.positions, "Production release is not on the newest release's develop history")
    return released, history


def enabled_components(policy, targets, runtime_targets):
    """Enabled applications, plus runtime images only under an explicit promotion switch.

    Production's private controller has no "runtime" switch, so it never
    selects a runtime image. Runtime images are Agent Manager settings, so
    even an explicit switch promotes them only together with AM.
    """
    runtimes = policy.get("runtime") is True and policy["am"]
    return sorted(c for c in targets if (runtimes if c in runtime_targets else policy[c]))


def select_versions(released, components, tags, history):
    """Newest published version per component whose source is the release commit or older."""
    limit = history.position(released)
    chosen = {}
    for component, prefix in components.items():
        eligible = [tag for tag in tags(component)
                    if tag["name"].startswith(prefix + "/v") and history.position(tag["target"]) <= limit]
        if not eligible:
            print(f"::error::No published {component} version is in the release commit's history", file=sys.stderr)
        require(bool(eligible), "A selected component has no published version in the release commit's history")
        newest = max(eligible, key=lambda tag: history.position(tag["target"]))
        chosen[component] = newest["name"][len(prefix + "/v"):]
    return chosen


def evidence_rows(archive):
    with zipfile.ZipFile(io.BytesIO(archive)) as source:
        names = source.namelist()
        require(names == ["deployment.json"], "Staging deployment evidence is malformed")
        info = source.getinfo("deployment.json")
        require(info.file_size <= MAX_EVIDENCE_BYTES, "Staging deployment evidence is oversized")
        value = json.loads(source.read("deployment.json"))
    require(isinstance(value, dict) and value.get("schema") == EVIDENCE_SCHEMA
            and isinstance(value.get("components"), dict), "Staging deployment evidence is malformed")
    return value["components"]


def trusted_view(run):
    return (isinstance(run, dict) and isinstance(run.get("id"), int) and run.get("head_branch") == "develop"
            and run.get("event") in ("push", "workflow_dispatch") and run.get("path") in EVIDENCE_WORKFLOWS
            and run.get("conclusion") in ("success", "failure")
            and isinstance(run.get("repository"), dict)
            and run["repository"].get("full_name") == "ChronoAIProject/cma")


def trusted_run(request, run_id):
    run = request(f"actions/runs/{run_id}")
    return trusted_view(run) and run.get("id") == run_id


def evidence_lane(name, run_id):
    """Return the lane suffix of this run's evidence artifact, '' for none, or None if foreign."""
    prefix = f"{EVIDENCE_PREFIX}{run_id}-"
    if not isinstance(name, str) or not name.startswith(prefix):
        return None
    attempt, _, lane = name[len(prefix):].partition("-")
    if not attempt.isdigit() or attempt.startswith("0"):
        return None
    # An automatic retry's evidence belongs to the same lane.
    return lane[: -len("-retry")] if lane.endswith("-retry") else lane


def newest_first(item):
    run = item[0]
    return -run["id"] if isinstance(run, dict) and isinstance(run.get("id"), int) else 0


def staging_verified(images, request, download, *, pointer_source=None, pointer_evidence=None):
    """Return the components whose exact image staging recorded as controller-verified."""
    pending, verified, downloads, listings = dict(images), set(), 0, 0
    if not pending:
        return verified
    # Read the workflows page by page together, newest run first within each
    # page round (not across rounds; acceptance never depends on order). Searching one
    # workflow to its end before the next starved redeploy-staging: its evidence
    # sat behind every staging CD run and the listing bound refused first.
    remaining = list(EVIDENCE_WORKFLOWS)
    for page in range(1, EVIDENCE_RUN_PAGES + 1):
        listed = []
        for workflow in list(remaining):
            name = workflow.rsplit("/", 1)[1]
            runs = request(f"actions/workflows/{name}/runs?branch=develop&status=completed&per_page=100&page={page}")
            require(isinstance(runs, dict) and isinstance(runs.get("workflow_runs"), list),
                    "Staging deployment evidence inventory is unavailable")
            listed.append([(run, workflow) for run in runs["workflow_runs"]])
            if len(runs["workflow_runs"]) < 100:
                remaining.remove(workflow)
        # Each listing is already newest first; merge them by run id and keep
        # each workflow's own order.
        for run, workflow in heapq.merge(*listed, key=newest_first):
            if not trusted_view(run) or run.get("path") != workflow:
                continue
            require(listings < MAX_RUN_LISTINGS, "Staging deployment evidence search exceeded its bound")
            listings += 1
            listing = request(f"actions/runs/{run['id']}/artifacts?per_page=100")
            require(isinstance(listing, dict) and isinstance(listing.get("artifacts"), list),
                    "Staging deployment evidence inventory is unavailable")
            for artifact in listing["artifacts"]:
                lane = evidence_lane(artifact.get("name"), run["id"])
                if lane is None or artifact.get("expired"):
                    continue
                if lane in COMPONENT_LANES and lane not in pending:
                    continue
                require(downloads < MAX_DOWNLOADS, "Staging deployment evidence search exceeded its bound")
                downloads += 1
                archive = download(artifact)
                for component, row in evidence_rows(archive).items():
                    if (component in pending and isinstance(row, dict) and row.get("state") == VERIFIED
                            and row.get("image") == pending[component]
                            and (component != "runtime" or pointer_source is None
                                 or (row.get("source_sha") == pointer_source and pointer_verified(component, row, "staging")))):
                        verified.add(component)
                        del pending[component]
                        if component == "runtime" and pointer_evidence is not None:
                            pointer_evidence.append(f"{run['id']}:{artifact['id']}:{hashlib.sha256(archive).hexdigest()}")
                if not pending:
                    return verified
        if not remaining:
            break
    return verified


def pointer_artifact(reference, request, download):
    require(matches(r"[1-9][0-9]*:[1-9][0-9]*:[0-9a-f]{64}", reference),
            "Production pointer activation requires immutable staging evidence")
    run_text, artifact_text, digest = reference.split(":")
    run_id, artifact_id = int(run_text), int(artifact_text)
    run = request(f"actions/runs/{run_id}")
    require(trusted_view(run) and run["id"] == run_id, "Staging pointer qualification workflow is untrusted")
    listing = request(f"actions/runs/{run_id}/artifacts?per_page=100")
    matches_id = [artifact for artifact in listing["artifacts"] if artifact.get("id") == artifact_id
                  and not artifact.get("expired") and evidence_lane(artifact.get("name"), run_id) is not None]
    require(len(matches_id) == 1, "Staging pointer qualification artifact is missing")
    archive = download(matches_id[0])
    require(hashlib.sha256(archive).hexdigest() == digest, "Staging pointer qualification changed")
    return run, evidence_rows(archive)


def initial_pointer_qualification(reference, request, download, *, tags, history):
    """The reviewed activation names an immutable staging two-lane artifact."""
    run, evidence = pointer_artifact(reference, request, download)
    require(run["conclusion"] == "success", "Initial staging pointer qualification workflow did not pass")
    lanes = ("runtime", "runtime-omarchy")
    require(all(pointer_verified(lane, evidence.get(lane), "staging", transition=True)
                for lane in lanes), "Initial staging qualification lacks both pointer promotions")
    require(all(evidence["runtime"]["pointer"][key] == evidence["runtime-omarchy"]["pointer"][key]
                for key in ("build", "build_digest")),
            "Initial staging qualification used different Agent Manager builds")
    for lane in lanes:
        row = evidence[lane]
        history.position(row["source_sha"])
        publications = [tag for tag in tags(lane) if tag.get("target") == row["source_sha"]
                        and isinstance(tag.get("receipt"), dict)
                        and tag["receipt"].get("source") == row["source_sha"]
                        and tag.get("name") == tag["receipt"].get("tag")
                        and row["image"] == tag["receipt"].get("repository", "") + "@" + tag["receipt"].get("digest", "")]
        require(len(publications) == 1, "Initial staging pointer promotion has no matching final publication")


def gh_download(token):
    def download(artifact):
        require(isinstance(artifact.get("id"), int), "Staging deployment evidence identity is invalid")
        env = {k: v for k, v in os.environ.items() if k in ("PATH", "HOME", "LANG", "LC_ALL")}
        env.update(GH_TOKEN=token, GH_HOST="github.com", GH_PROMPT_DISABLED="1")
        with tempfile.TemporaryDirectory() as directory:
            path = Path(directory) / "evidence.zip"
            for attempt in range(DOWNLOAD_ATTEMPTS):
                # A transient download failure is retried a bounded number of
                # times; a persistent one still refuses the whole selection.
                with path.open("wb") as stream:
                    result = subprocess.run(
                        ["gh", "api", f"repos/ChronoAIProject/cma/actions/artifacts/{artifact['id']}/zip"],
                        env=env, stdout=stream, stderr=subprocess.DEVNULL, timeout=60, check=False)
                if result.returncode == 0 or attempt == DOWNLOAD_ATTEMPTS - 1:
                    break
                time.sleep(2 * (attempt + 1))
            require(result.returncode == 0 and path.stat().st_size <= 4 * MAX_EVIDENCE_BYTES,
                    "Staging deployment evidence could not be downloaded")
            return path.read_bytes()
    return download


def plan(env, api, registry, policy, *, root, download, deployables, runner=subprocess.run, pointer_evidence=None):
    """Return (release_set or None, release commit) for the production event in env."""
    from staging_cd_admission import event_identity, maintainer_dispatch
    from staging_cd_receipts import canonical
    from staging_cd_resolve import resolve
    from staging_cd_set import dispatch_set, release_set
    from staging_cd_targets import RUNTIME_TARGETS, TARGETS

    require(env.get("CD_TARGET") == "production", "Production selection requires the production target")
    if env.get("GITHUB_EVENT_NAME") == "workflow_dispatch":
        # Like a staging redeploy, a retry needs maintainer actors, before and after approval.
        maintainer_dispatch(env, api.request)
    else:
        event_identity(env)
    released = release_commit(env["GITHUB_SHA"], root, runner=runner)
    components = enabled_components(policy, TARGETS, RUNTIME_TARGETS)
    pointers = env.get("CD_RUNTIME_POINTER_PUBLICATION_ENABLED") == "true"
    if not components and not pointers:
        return None, released
    history = api.history()
    if components:
        versions = select_versions(released, {c: deployables[c] for c in components}, api.tags, history)
        inputs = {"deployables": ",".join(components), "versions": canonical(versions), "mode": "release"}
        _, value = dispatch_set(inputs, api, registry)
    if pointers:
        initial = env.get("CD_RUNTIME_POINTER_STAGING_QUALIFICATION")
        approved = env.get("CD_APPROVED_POINTER_EVIDENCE")
        if "CD_RELEASE_SET" in env:
            require(isinstance(approved, str) and approved.count("/") == 1,
                    "The production approval has no bound pointer evidence")
            approved_initial, approved_promotion = approved.split("/")
            require(approved_initial == initial, "The initial staging qualification differs from production approval")
        else:
            approved_promotion = None
        initial_pointer_qualification(initial, api.request, download, tags=api.tags, history=history)
        if pointer_evidence is not None:
            pointer_evidence.append(initial)
        # Only finals on the approved production release are eligible, even
        # while newer runtimes continue publishing on develop.
        limit = history.position(released)
        tags = [tag for tag in api.tags("runtime") if history.position(tag["target"]) <= limit]
        runtime = resolve("runtime", "latest", tags, registry, history)
        value = release_set((value["receipts"] if components else {}) | {"runtime": runtime})
    # The same image string staging_cd_set gives the controller and its evidence records.
    images = {c: r["repository"] + "@" + r["digest"] for c, r in value["receipts"].items()}
    if pointers:
        if approved_promotion is not None:
            _, evidence = pointer_artifact(approved_promotion, api.request, download)
            row = evidence.get("runtime")
            require(isinstance(row, dict) and row.get("image") == images["runtime"]
                    and row.get("source_sha") == runtime["source"] and pointer_verified("runtime", row, "staging"),
                    "The approved staging pointer promotion differs from the release")
            verified = staging_verified({key: image for key, image in images.items() if key != "runtime"},
                                        api.request, download) | {"runtime"}
        else:
            options = {"pointer_source": runtime["source"]}
            if pointer_evidence is not None:
                options["pointer_evidence"] = pointer_evidence
            verified = staging_verified(images, api.request, download, **options)
    else:
        verified = staging_verified(images, api.request, download)
    missing = sorted(set(images) - verified)
    if missing:
        print("::error::Staging never verified the released image for: " + ", ".join(missing), file=sys.stderr)
    require(not missing, "Staging never verified a released image")
    return value, released


def main():
    """Plan in the resolve job; in the deploy job, after approval, prove the plan still holds.

    The deploy job passes the approved set as CD_RELEASE_SET. A production deploy
    waits for a human approval, so its admission clock starts when this
    re-selection reproduces the identical set, not when the run was created.
    The re-selection covers exactly the approved components; the controller
    invocation then applies production's deploy switches as the final word.
    Runtime pointer publication must remain enabled across approval. Omarchy
    publications may be qualified in staging and are excluded from production.
    """
    from staging_cd_clock import timestamp
    from staging_cd_github import GitHub
    from staging_cd_receipts import canonical
    from staging_cd_registry import Registry
    from staging_cd_resolve import output
    from staging_cd_runtime import run_private
    from staging_cd_set import document
    from staging_cd_targets import APP_TARGETS, RUNTIME_TARGETS
    from staging_cd_versions import DEPLOYABLES

    env = os.environ

    def operation(_masker):
        approved = env.get("CD_RELEASE_SET")
        if approved is None:
            from staging_cd_policy import switches  # PyYAML is installed only in the resolve job

            policy = switches(Path(env["CD_PRIVATE_CONTROLLER"]) / "scripts" / "deploy_set.py")
        else:
            components = set(document(approved)["receipts"])
            runtime = components & RUNTIME_TARGETS
            require(not runtime or (runtime == {"runtime"}
                                    and env.get("CD_RUNTIME_POINTER_PUBLICATION_ENABLED") == "true"),
                    "An approved production release set must not hold a runtime image")
            policy = {key: key in components for key in sorted(APP_TARGETS)}
        api, registry = GitHub(env.get("GH_TOKEN")), Registry()
        pointer_evidence = []
        value, released = plan(env, api, registry, policy, root=Path.cwd(),
                               download=gh_download(env["GH_TOKEN"]), deployables=DEPLOYABLES,
                               pointer_evidence=pointer_evidence)
        output(env, "released", released)
        if approved is not None:
            require(value is not None and canonical(value) == canonical(document(approved)),
                    "The approved production release set no longer matches its selection")
            output(env, "started_at", timestamp())
        elif value is None:
            output(env, "release_set", "")
            return True
        elif pointer_evidence:
            require(len(pointer_evidence) == 2, "Selected Agent promotion has no unique staging evidence")
            output(env, "pointer_evidence", "/".join(pointer_evidence))
        output(env, "release_set", canonical(value))
        output(env, "mode", "release")
        return True

    return run_private(operation, None, subject="production-release")


if __name__ == "__main__":
    raise SystemExit(main())
