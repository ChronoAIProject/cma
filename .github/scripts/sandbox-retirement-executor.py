#!/usr/bin/env python3
"""Execute one reviewed, ordered retirement inventory through the CMA API."""

import argparse
from datetime import datetime, timezone
from hashlib import sha256
import json
import os
from pathlib import Path
import re
import runpy
import signal
import stat
import subprocess

from sandbox_retirement_application import ApplicationRetirement, JsonTransport, ProviderReads, digest
from sandbox_retirement_execution import execute
from sandbox_retirement_journal import Refused, RetirementJournal, digest as is_digest


DIRECTORY = Path(__file__).resolve().parent
SOURCE = (
    "sandbox-retirement-executor.py",
    "sandbox-retirement-preflight.py",
    "sandbox_retirement_application.py",
    "sandbox_retirement_execution.py",
    "sandbox_retirement_finality.py",
    "sandbox_retirement_journal.py",
)
DNS = re.compile(r"[a-z0-9](?:[-.a-z0-9]{0,251}[a-z0-9])?\Z")
GIT_SHA = re.compile(r"[0-9a-f]{40}\Z")
VERSION = re.compile(r"v?\d+\.\d+\.\d+(?:-[a-zA-Z0-9.-]+)?(?:\+[a-zA-Z0-9.-]+)?\Z")
RELEASE_CONTROLLER = Path("/usr/local/libexec/cma-retirement-release-controller")


class ExecutionDeadline(RuntimeError):
    pass


def source_digest():
    return digest({filename: sha256((DIRECTORY / filename).read_bytes()).hexdigest() for filename in SOURCE})


def provider_identity(obj):
    return (
        isinstance(obj, dict)
        and set(obj) == {"name", "uid"}
        and all(isinstance(value, str) and DNS.fullmatch(value) for value in obj.values())
    )


def parse_provider(value):
    resources = {"sandbox", "claim", "pvc", "pv"}
    if (
        not isinstance(value, dict)
        or set(value) != resources | {"namespace", "pods", "storage"}
        or not isinstance(value["namespace"], str)
        or not DNS.fullmatch(value["namespace"])
        or value["storage"] not in {"retain_bound", "retain_released"}
        or not all(provider_identity(value[kind]) for kind in resources)
        or not isinstance(value["pods"], list)
        or len(value["pods"]) > 32
        or not all(provider_identity(pod) for pod in value["pods"])
        or len({pod["name"] for pod in value["pods"]}) != len(value["pods"])
    ):
        raise Refused("invalid provider inventory")
    return value


def parse_inventory(raw, preflight, now):
    value = json.loads(raw, object_pairs_hook=preflight["unique_fields"])
    if (
        not isinstance(value, dict) or set(value) != {"format", "idle_before", "members"}
        or type(value["format"]) is not int or value["format"] != 1 or not isinstance(value["members"], list)
    ):
        raise Refused("invalid execution inventory")
    for row in value["members"]:
        if not isinstance(row, dict) or set(row) != {"snapshot", "provider"}:
            raise Refused("invalid member")
        parse_provider(row["provider"])
    inventory = {**value, "members": [row["snapshot"] for row in value["members"]]}
    cutoff, _ = preflight["parse_inventory"](json.dumps(inventory).encode(), now)
    members = []
    for row in value["members"]:
        members.append({
            **row["snapshot"], **row, "idle_before": value["idle_before"],
            "member_sha256": digest(row),
        })
    return cutoff, members


def read_review(raw, preflight, inventory_sha256, origins, state_dir):
    review = json.loads(raw, object_pairs_hook=preflight["unique_fields"])
    expected = {
        "format": 1, "verdict": "GO", "executor_sha256": source_digest(),
        "inventory_sha256": inventory_sha256, "origin_sha256": digest(origins),
        "state_dir_sha256": digest(str(state_dir)),
    }
    if (
        not isinstance(review, dict)
        or set(review) != set(expected) | {"dispatch_before", "approval_sha256", "serving_release", "resume_index"}
        or type(review["format"]) is not int
        or type(review["resume_index"]) is not int or review["resume_index"] < 0
        or not all(review[field] == value for field, value in expected.items())
        or not is_digest(review["approval_sha256"])
    ):
        raise Refused("review does not match executor, inventory, origins and journal location")
    release = review["serving_release"]
    revisions = ("source_sha", "admission_floor_sha")
    if (
        not isinstance(release, dict)
        or set(release) != {*revisions, "version", "rollout_evidence_sha256"}
        or not all(isinstance(release[key], str) and GIT_SHA.fullmatch(release[key]) for key in revisions)
        or not isinstance(release["version"], str) or not VERSION.fullmatch(release["version"])
        or not is_digest(release["rollout_evidence_sha256"])
    ):
        raise Refused("reviewed serving release is invalid")
    expires = preflight["utc_timestamp"](review["dispatch_before"])
    return review, expires


def credential(name):
    value = os.environ.get(name, "")
    if not value or any(character.isspace() for character in value):
        raise Refused("required credential unavailable")
    return value


def consume_release_approval(request):
    """Use the release owner's installed controller adapter."""
    for path in (RELEASE_CONTROLLER, *RELEASE_CONTROLLER.parents):
        info = path.lstat()
        expected = stat.S_ISREG if path == RELEASE_CONTROLLER else stat.S_ISDIR
        if info.st_uid != 0 or info.st_mode & 0o022 or not expected(info.st_mode):
            raise Refused("release-controller installation is not trusted")
    result = subprocess.run(
        [RELEASE_CONTROLLER], input=json.dumps(request).encode(), capture_output=True,
        timeout=15, check=False, env={"PATH": "/usr/bin:/bin"},
    )
    if result.returncode != 0 or len(result.stdout) > 4096:
        return False
    return json.loads(result.stdout) == {
        "status": "approved", "context": request["context"],
        "serving_release": request["serving_release"],
    }


class SingleUseApproval:
    def __init__(self, consume, app, token, review, expires):
        self.consume = consume
        self.app = app
        self.token = token
        self.release = review["serving_release"]
        # The issuer allows at most 200 characters. The digest binds all
        # reviewed fields except the token that is issued for that context.
        authority = digest({key: value for key, value in review.items() if key != "approval_sha256"})
        self.context = f"old-sandbox-retire/{authority}"
        self.expires = expires
        self.consumed = False

    def __call__(self):
        if datetime.now(timezone.utc) >= self.expires:
            return False
        status, build = self.app.read("/api/v1/build-info", 5)
        if status != 200 or build != {key: self.release[key] for key in ("source_sha", "version")}:
            return False
        if not self.consumed:
            self.consumed = self.consume({
                "approval": self.token, "context": self.context, "origin": self.app.origin,
                "serving_release": self.release,
            })
        return self.before_send()

    def before_send(self):
        return self.consumed and datetime.now(timezone.utc) < self.expires


def run(args, preflight, consume_approval):
    now = datetime.now(timezone.utc)
    origins = {
        "application": preflight["admin_origin"](args.application),
        "provider": preflight["admin_origin"](args.provider),
    }
    read = preflight["read_inventory"]
    raw_inventory = read(args.inventory)
    inventory_sha256 = sha256(raw_inventory).hexdigest()
    cutoff, members = parse_inventory(raw_inventory, preflight, now)
    state_dir = args.state_dir.resolve(strict=True)
    review, expires = read_review(read(args.review), preflight, inventory_sha256, origins, state_dir)
    stable_review_sha256 = digest({
        key: value for key, value in review.items()
        if key not in {"approval_sha256", "dispatch_before", "resume_index"}
    })
    journal_path = state_dir / f"{inventory_sha256}.jsonl"
    resume_index = review["resume_index"]
    if args.execute:
        approval = credential("CMA_RETIREMENT_APPROVAL")
        if sha256(approval.encode()).hexdigest() != review["approval_sha256"] or expires <= now:
            raise Refused("fresh matching release approval is required")

    app = JsonTransport(origins["application"], credential("CMA_RETIREMENT_APP_TOKEN"))
    authority = (
        SingleUseApproval(consume_approval, app, approval, review, expires)
        if args.execute else None
    )
    provider = ProviderReads(JsonTransport(origins["provider"], credential("CMA_RETIREMENT_PROVIDER_TOKEN")))
    boundary = ApplicationRetirement(
        app, provider, inventory_sha256, cutoff, preflight["inspect_member"], args.observe_seconds,
    )
    with RetirementJournal(
        journal_path, stable_review_sha256, inventory_sha256, create=args.execute and resume_index == 0,
    ) as journal:
        if args.execute and (journal.next_index != resume_index or journal.pending is not None):
            raise Refused("approval does not start at the next unreserved member")
        finished = execute(members, journal, boundary, authority)
        return {
            "inventory_sha256": inventory_sha256, "completed_members": journal.next_index,
            "outcome": "completed" if finished else "unresolved",
        }


def main(consume_approval=consume_release_approval):
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--application", required=True)
    parser.add_argument("--provider", required=True)
    parser.add_argument("--inventory", required=True, type=Path)
    parser.add_argument("--review", required=True, type=Path)
    parser.add_argument("--state-dir", required=True, type=Path)
    authority = parser.add_mutually_exclusive_group(required=True)
    authority.add_argument("--observe-only", action="store_true")
    authority.add_argument("--execute", action="store_true")
    parser.add_argument("--observe-seconds", type=int, default=60, choices=range(1, 301), metavar="SECONDS")
    args = parser.parse_args()
    preflight = runpy.run_path(str(DIRECTORY / "sandbox-retirement-preflight.py"))
    signal.signal(signal.SIGALRM, execution_timeout)
    signal.alarm(900)
    try:
        report = run(args, preflight, consume_approval)
        print(json.dumps(report, sort_keys=True))
        return int(report["outcome"] == "unresolved")
    except (OSError, ValueError, KeyError, TypeError, ExecutionDeadline, subprocess.SubprocessError):
        parser.exit(2, "Retirement execution refused or interrupted. Inspect the private journal.\n")
    finally:
        signal.alarm(0)


def execution_timeout(*_):
    raise ExecutionDeadline("retirement execution deadline")


if __name__ == "__main__":
    raise SystemExit(main())
