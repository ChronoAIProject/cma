"""Reserve repository cache headroom for a sealed BuildKit layer archive."""

import argparse
import json
import os
import re
import stat
from dataclasses import dataclass
from pathlib import Path

from actions_cache_retention import BUDGET, CARGO, DEVELOP
from actions_cache_sweep import Actions
from actions_cargo_budget import BudgetRef, MAX_CHARGE as CARGO_MAX_CHARGE, SCHEMA as CARGO_SCHEMA, stored_bound
from buildkit_cache_archive import MAX_BYTES, digest, inventory as validate_archive
from staging_cd_common import require
from staging_cd_versions import REPOSITORY


PREFIX = "cma-layer-archive-v1-"
KEY = re.compile(PREFIX + r"(am|eg|web|runtime|runtime-omarchy)--([1-9][0-9]*)-([1-9][0-9]*)")
SCHEMA = "shared-cache-budget-v1"
MAX_CHARGE = stored_bound(MAX_BYTES + 4096)


@dataclass
class Account:
    charges: dict

    @classmethod
    def decode(cls, doc):
        require(isinstance(doc, dict) and set(doc) == {"schema", "charges"}
                and doc["schema"] in (CARGO_SCHEMA, SCHEMA),
                "Invalid layer cache budget format")
        charges = doc["charges"]
        require(isinstance(charges, dict) and len(charges) <= 1024, "Invalid layer cache budget charges")
        for key, limit in charges.items():
            cap = CARGO_MAX_CHARGE if CARGO.fullmatch(key) else MAX_CHARGE if KEY.fullmatch(key) else 0
            require(type(limit) is int and 0 < limit <= cap
                    and (doc["schema"] == SCHEMA or CARGO.fullmatch(key)),
                    "Invalid layer cache charge")
        return cls(charges)

    def encode(self):
        return {"schema": SCHEMA, "charges": self.charges}


def archive_key(component, run):
    key = PREFIX + f"{component}--{run['id']}-{run['run_attempt']}"
    require(KEY.fullmatch(key), "Invalid layer cache archive key")
    return key


def source_run(env):
    require(env.get("GITHUB_REPOSITORY") == REPOSITORY and env.get("GITHUB_REF") == DEVELOP
            and env.get("GITHUB_EVENT_NAME") == "workflow_run"
            and env.get("GITHUB_WORKFLOW_REF") == f"{REPOSITORY}/.github/workflows/cache-retention.yml@{DEVELOP}"
            and env.get("CD_LAYER_ARCHIVE_CACHE") == "enabled"
            and env.get("CD_LAYER_CACHE_WRITER_FLOOR") == "verified", "Layer cache authority differs")
    event = json.loads(Path(env["GITHUB_EVENT_PATH"]).read_text())
    run = event["workflow_run"]
    require(event["action"] == "completed" and run["status"] == "completed" and run["event"] == "push"
            and run["path"] == ".github/workflows/staging-cd.yml" and run["head_branch"] == "develop"
            and run["head_repository"]["full_name"] == REPOSITORY
            and run["repository"]["full_name"] == REPOSITORY
            and type(run["id"]) is int and run["id"] > 0
            and type(run["run_attempt"]) is int and run["run_attempt"] > 0
            and re.fullmatch(r"[a-f0-9]{40}", run["head_sha"]), "Invalid source publication")
    return run


def select(api, run, component):
    archive_key(component, run)
    run_id, attempt = run["id"], run["run_attempt"]
    name = f"cma-layer-cache-{component}-{run_id}-{attempt}"
    artifacts = api.list(f"runs/{run_id}/artifacts", "artifacts", name=name)
    if not artifacts:
        return None
    require(len(artifacts) == 1, "Ambiguous archive candidate")
    artifact = artifacts[0]
    require(artifact["name"] == name and artifact["expired"] is False
            and type(artifact["id"]) is int and artifact["id"] > 0
            and type(artifact["size_in_bytes"]) is int and 0 < artifact["size_in_bytes"] <= MAX_BYTES + 1024**2
            and re.fullmatch(r"sha256:[a-f0-9]{64}", artifact.get("digest", "")),
            "Invalid sealed archive artifact")
    jobs = api.list(f"runs/{run_id}/attempts/{attempt}/jobs", "jobs")
    matches = [job for job in jobs if job["name"] == f"Release {component} / Publish"]
    require(len(matches) == 1, "Archive publication is ambiguous")
    job = matches[0]
    require(job["run_id"] == run_id and job["run_attempt"] == attempt and job["head_sha"] == run["head_sha"]
            and job["status"] == "completed" and job["conclusion"] == "success", "Archive publication did not pass")
    seals = [step for step in job["steps"] if step["name"] == "Retain sealed layer archive for admission"]
    require(len(seals) == 1 and seals[0]["conclusion"] == "success", "Archive was not accepted")
    return artifact["id"]


def select_am_receipt(api, run):
    run_id, attempt = run["id"], run["run_attempt"]
    name = f"staging-receipts-{run_id}-{attempt}-am"
    artifacts = api.list(f"runs/{run_id}/artifacts", "artifacts", name=name)
    if not artifacts:
        return None
    require(len(artifacts) == 1, "Ambiguous AM publication receipt")
    artifact = artifacts[0]
    require(artifact["name"] == name and artifact["expired"] is False
            and type(artifact["id"]) is int and artifact["id"] > 0
            and type(artifact["size_in_bytes"]) is int and 0 < artifact["size_in_bytes"] <= 1024**2
            and re.fullmatch(r"sha256:[a-f0-9]{64}", artifact.get("digest", "")), "Invalid AM receipt artifact")
    jobs = api.list(f"runs/{run_id}/attempts/{attempt}/jobs", "jobs")
    for name, steps in (
            ("Release am / Publish", {"Retain only public publication receipts", "Retry retaining only public publication receipts"}),
            ("Release am / Finalize", {"Publish digest-bound final tags and remove successful reservations"})):
        matches = [job for job in jobs if job["name"] == name]
        if len(matches) != 1:
            return None
        job = matches[0]
        if not (job["run_id"] == run_id and job["run_attempt"] == attempt and job["head_sha"] == run["head_sha"]
                and job["status"] == "completed" and job["conclusion"] == "success"
                and any(step["name"] in steps and step["conclusion"] == "success" for step in job["steps"])):
            return None
    return artifact["id"]


def reserve(api, ledger, key, archive, revision, *, replay_bytes=0):
    require(KEY.fullmatch(key), "Invalid archive key")
    info = archive.lstat()
    require(stat.S_ISREG(info.st_mode) and info.st_nlink == 1 and 0 < info.st_size <= MAX_BYTES,
            "Layer archive exceeds byte bound")
    require(type(replay_bytes) is int and 0 <= replay_bytes <= 4096, "Layer replay metadata exceeds bound")
    cap = stored_bound(info.st_size + replay_bytes)
    api.require_live_policy(revision)
    previous, account = ledger.read()
    initial = dict(account.charges)
    entries = api.entries()
    relevant = set(account.charges) | {key}
    visible = {}
    for entry in entries:
        if entry.ref == DEVELOP and entry.size and entry.key in relevant:
            require(entry.key not in visible, "Ambiguous reserved cache generation")
            visible[entry.key] = entry.size
    for pending, limit in list(account.charges.items()):
        if pending in visible:
            require(visible[pending] <= limit, "Stored layer cache exceeded its reservation")
            del account.charges[pending]
    # Migrate in the original Cargo ref using the same fast-forward mutation.
    # An old writer that read the previous revision loses the ref update; an
    # old reader of the new schema refuses. Existing charges survive migration.
    observed = sum(entry.size for entry in entries)
    outstanding = sum(account.charges.values())
    allowed = key not in visible and key not in account.charges and observed + outstanding + cap < BUDGET
    if allowed:
        account.charges[key] = cap
    if ledger.read()[0] != previous:
        return {"allowed": False, "reason": "budget-revision-changed"}
    if account.charges != initial:
        api.require_live_policy(revision)
        ledger.advance(previous, account, revision)
    return {"allowed": allowed, "observed_bytes": observed, "outstanding_bytes": outstanding,
            "archive_bound_bytes": cap, "budget_bytes": BUDGET}


def retire_verified(api, key, revision, restored):
    """Retire only our own archive generations after a fresh exact restore."""
    validate_archive(restored)
    component = KEY.fullmatch(key)[1]
    api.require_live_policy(revision)
    entries = api.entries()
    matches = [entry for entry in entries if entry.ref == DEVELOP and entry.key == key and entry.size]
    require(len(matches) == 1, "Verified cache identity is ambiguous")
    keep = matches[0]
    require(keep.size <= MAX_CHARGE, "Verified cache exceeds stored size bound")
    family = PREFIX + component + "--"
    older = [entry for entry in entries if entry.ref == DEVELOP and entry.key.startswith(family)
             and KEY.fullmatch(entry.key) and KEY.fullmatch(entry.key)[1] == component
             and entry.version == keep.version and entry.created < keep.created]
    # New readers always restore the newest compatible prefix. Active readers
    # can already have selected an older immutable key, including PR readers
    # and readers on branches which fall back to the default branch.
    deleted = 0
    for entry in sorted(older, key=lambda row: (row.created, row.id)):
        if any(run["id"] != api.run_id for run in api.active_runs()):
            break
        if not api.visible(keep) or api.visible(entry) != entry:
            break
        api.require_live_policy(revision)
        print(json.dumps({"action": "retire-verified-predecessor", "id": entry.id, "successor": keep.id}), flush=True)
        if api.delete(entry.id):
            deleted += entry.size
    final = api.entries()
    require(any(row.id == keep.id and row.key == keep.key and row.version == keep.version for row in final),
            "Verified successor is no longer visible")
    return {"verified_cache_id": keep.id, "verified_cache_version": keep.version,
            "verified_stored_bytes": keep.size, "deleted_bytes": deleted,
            "before_bytes": sum(row.size for row in entries), "after_bytes": sum(row.size for row in final)}


def main(env, mode):
    run = source_run(env)
    component = env["CD_COMPONENT"]
    archive = Path(env["RUNNER_TEMP"]) / "cma-layer-cache" / "archive.tar"
    api = Actions(env["GH_TOKEN"], int(env["GITHUB_RUN_ID"]))
    key = archive_key(component, run)
    if mode == "select":
        artifact = select_am_receipt(api, run) if component == "am" else select(api, run, component)
        if artifact is not None:
            with Path(env["GITHUB_OUTPUT"]).open("a") as output:
                output.write(f"artifact={artifact}\nrun_id={run['id']}\n")
        return
    if mode == "verify":
        require(env.get("LAYER_EXACT_CACHE_HIT") == "true", "Fresh exact layer restore is required")
        with archive.open("rb") as stream:
            require(digest(stream) == env.get("LAYER_REUSE_ARCHIVE_DIGEST"),
                    "BuildKit reuse proof does not match the restored archive")
        result = retire_verified(api, key, env["GITHUB_SHA"], archive.parent / "verified")
        print(json.dumps(result), flush=True)
        return
    ledger = BudgetRef(api.api, account_type=Account)
    from staging_cd_layer_reuse import read_replay
    replay = archive.with_name("replay.json")
    value = read_replay(replay, sealed=True)
    reservation = value["reservation"]
    require((reservation["component"], reservation["run_id"], reservation["run_attempt"])
            == (component, run["id"], run["run_attempt"]), "Layer candidate replay differs")
    with archive.open("rb") as stream:
        require(digest(stream) == value["archive_digest"], "Layer candidate archive differs")
    result = reserve(api, ledger, key, archive, env["GITHUB_SHA"], replay_bytes=replay.stat().st_size)
    with Path(env["GITHUB_OUTPUT"]).open("a") as output:
        output.write(f"key={key}\nallowed={str(result['allowed']).lower()}\n")
    print(json.dumps(result), flush=True)


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("mode", choices=("select", "reserve", "verify"))
    main(os.environ, parser.parse_args().mode)
