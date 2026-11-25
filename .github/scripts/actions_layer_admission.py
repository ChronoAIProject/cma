"""Reserve repository cache headroom for a sealed BuildKit layer archive."""

import argparse
import json
import os
import re
import stat
from dataclasses import dataclass
from pathlib import Path

from actions_cache_retention import BUDGET, DEVELOP
from actions_cache_sweep import Actions
from actions_cargo_budget import BudgetRef, stored_bound
from buildkit_cache_archive import MAX_BYTES
from staging_cd_common import require
from staging_cd_versions import REPOSITORY


PREFIX = "cma-layer-archive-v1-"
KEY = re.compile(PREFIX + r"(am|eg|web|runtime|runtime-omarchy)-([1-9][0-9]*)-([1-9][0-9]*)")
REF = "heads/automation/layer-cache-budget-v1"
SCHEMA = "layer-cache-budget-v1"
HEADER = "Account for outstanding develop layer archives\n\n"
MAX_CHARGE = stored_bound(MAX_BYTES)


@dataclass
class Account:
    charges: dict

    @classmethod
    def decode(cls, doc):
        require(isinstance(doc, dict) and set(doc) == {"schema", "charges"} and doc["schema"] == SCHEMA,
                "Invalid layer cache budget format")
        charges = doc["charges"]
        require(isinstance(charges, dict) and len(charges) <= 1024, "Invalid layer cache budget charges")
        for key, limit in charges.items():
            require(KEY.fullmatch(key) and type(limit) is int and 0 < limit <= MAX_CHARGE,
                    "Invalid layer cache charge")
        return cls(charges)

    def encode(self):
        return {"schema": SCHEMA, "charges": self.charges}


def archive_key(component, run):
    key = PREFIX + f"{component}-{run['id']}-{run['run_attempt']}"
    require(KEY.fullmatch(key), "Invalid layer cache archive key")
    return key


def source_run(env):
    require(env.get("GITHUB_REPOSITORY") == REPOSITORY and env.get("GITHUB_REF") == DEVELOP
            and env.get("GITHUB_EVENT_NAME") == "workflow_run"
            and env.get("GITHUB_WORKFLOW_REF") == f"{REPOSITORY}/.github/workflows/cache-retention.yml@{DEVELOP}"
            and env.get("CD_LAYER_ARCHIVE_CACHE") == "enabled", "Layer cache authority differs")
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


def reserve(api, ledger, cargo_ledger, key, archive, revision):
    require(KEY.fullmatch(key), "Invalid archive key")
    info = archive.lstat()
    require(stat.S_ISREG(info.st_mode) and info.st_nlink == 1 and 0 < info.st_size <= MAX_BYTES,
            "Layer archive exceeds byte bound")
    cap = stored_bound(info.st_size)
    api.require_live_policy(revision)
    previous, account = ledger.read()
    cargo_revision, cargo = cargo_ledger.read()
    initial = dict(account.charges)
    entries = api.entries()
    relevant = set(account.charges) | set(cargo.charges) | {key}
    visible = {}
    for entry in entries:
        if entry.ref == DEVELOP and entry.size and entry.key in relevant:
            require(entry.key not in visible, "Ambiguous reserved cache generation")
            visible[entry.key] = entry.size
    for pending, limit in list(account.charges.items()):
        if pending in visible:
            require(visible[pending] <= limit, "Stored layer cache exceeded its reservation")
            del account.charges[pending]
    # A previous Cargo writer can still finish a reserved upload. Count its
    # outstanding reservation until the corresponding entry becomes visible.
    for pending, limit in cargo.charges.items():
        require(visible.get(pending, 0) <= limit, "Stored Cargo archive exceeded its reservation")
    cargo_pending = sum(limit for pending, limit in cargo.charges.items() if pending not in visible)
    observed = sum(entry.size for entry in entries)
    outstanding = sum(account.charges.values()) + cargo_pending
    allowed = key not in visible and key not in account.charges and observed + outstanding + cap < BUDGET
    if allowed:
        account.charges[key] = cap
    if ledger.read()[0] != previous or cargo_ledger.read()[0] != cargo_revision:
        return {"allowed": False, "reason": "budget-revision-changed"}
    if account.charges != initial:
        api.require_live_policy(revision)
        ledger.advance(previous, account, revision)
    return {"allowed": allowed, "observed_bytes": observed, "outstanding_bytes": outstanding,
            "archive_bound_bytes": cap, "budget_bytes": BUDGET}


def retire_verified(api, key, revision):
    """Retire only our own archive generations after a fresh exact restore."""
    component = KEY.fullmatch(key)[1]
    api.require_live_policy(revision)
    entries = api.entries()
    matches = [entry for entry in entries if entry.ref == DEVELOP and entry.key == key and entry.size]
    require(len(matches) == 1, "Verified cache identity is ambiguous")
    keep = matches[0]
    require(keep.size <= MAX_CHARGE, "Verified cache exceeds stored size bound")
    family = PREFIX + component + "-"
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
        artifact = select(api, run, component)
        if artifact is not None:
            with Path(env["GITHUB_OUTPUT"]).open("a") as output:
                output.write(f"artifact={artifact}\nrun_id={run['id']}\n")
        return
    if mode == "verify":
        require(env.get("LAYER_EXACT_CACHE_HIT") == "true", "Fresh exact layer restore is required")
        result = retire_verified(api, key, env["GITHUB_SHA"])
        print(json.dumps(result), flush=True)
        return
    ledger = BudgetRef(api.api, ref=REF, account_type=Account, header=HEADER)
    result = reserve(api, ledger, BudgetRef(api.api), key, archive, env["GITHUB_SHA"])
    with Path(env["GITHUB_OUTPUT"]).open("a") as output:
        output.write(f"key={key}\nallowed={str(result['allowed']).lower()}\n")
    print(json.dumps(result), flush=True)


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("mode", choices=("select", "reserve", "verify"))
    main(os.environ, parser.parse_args().mode)
