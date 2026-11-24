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
PRIVATE_ENV = {"CMA_RETIREMENT_APP_TOKEN", "CMA_RETIREMENT_PROVIDER_TOKEN", "CMA_RETIREMENT_APPROVAL"}


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


def read_review(raw, preflight, inventory_sha256, origins):
    review = json.loads(raw, object_pairs_hook=preflight["unique_fields"])
    expected = {
        "format": 1, "verdict": "GO", "executor_sha256": source_digest(),
        "inventory_sha256": inventory_sha256, "origin_sha256": digest(origins),
    }
    if (
        not isinstance(review, dict)
        or set(review) != set(expected) | {"dispatch_before", "approval_sha256"}
        or type(review["format"]) is not int
        or not all(review[field] == value for field, value in expected.items())
        or not is_digest(review["approval_sha256"])
    ):
        raise Refused("review does not match executor, inventory and origins")
    expires = preflight["utc_timestamp"](review["dispatch_before"])
    return review, expires


def credential(name):
    value = os.environ.get(name, "")
    if not value or any(character.isspace() for character in value):
        raise Refused("required credential unavailable")
    return value


class SingleUseApproval:
    def __init__(self, tool, token, context, expires):
        self.tool = tool
        self.token = token
        self.context = context
        self.expires = expires
        self.consumed = False

    def __call__(self):
        if datetime.now(timezone.utc) >= self.expires:
            return False
        if not self.consumed:
            result = subprocess.run(
                [self.tool, "policy-check", "deploy", "--approval", self.token, "--context", self.context],
                capture_output=True, timeout=15, check=False,
                env={key: value for key, value in os.environ.items() if key not in PRIVATE_ENV},
            )
            self.consumed = (
                result.returncode == 0
                and result.stdout.strip() == b"ALLOWED once via reviewed-release-controller"
            )
        return self.consumed and datetime.now(timezone.utc) < self.expires


def run(args, preflight):
    now = datetime.now(timezone.utc)
    origins = {
        "application": preflight["admin_origin"](args.application),
        "provider": preflight["admin_origin"](args.provider),
    }
    read = preflight["read_inventory"]
    raw_inventory = read(args.inventory)
    inventory_sha256 = sha256(raw_inventory).hexdigest()
    cutoff, members = parse_inventory(raw_inventory, preflight, now)
    review, expires = read_review(read(args.review), preflight, inventory_sha256, origins)
    journal_path = args.state_dir / f"{inventory_sha256}.jsonl"
    if args.observe_only:
        dispatch_allowed = lambda: False
    else:
        approval = credential("CMA_RETIREMENT_APPROVAL")
        if sha256(approval.encode()).hexdigest() != review["approval_sha256"] or expires <= now:
            raise Refused("fresh matching release approval is required")
        # Hash the immutable review context separately from the token digest,
        # so the release owner can issue an approval for a known context.
        authority = digest({key: val for key, val in review.items() if key != "approval_sha256"})
        context = f"old-sandbox-retire/{review['executor_sha256']}/{inventory_sha256}/{authority}"
        dispatch_allowed = SingleUseApproval(args.policy_tool, approval, context, expires)

    app = JsonTransport(origins["application"], credential("CMA_RETIREMENT_APP_TOKEN"))
    provider = ProviderReads(JsonTransport(origins["provider"], credential("CMA_RETIREMENT_PROVIDER_TOKEN")))
    boundary = ApplicationRetirement(
        app, provider, inventory_sha256, cutoff, preflight["inspect_member"], args.observe_seconds,
    )
    with RetirementJournal(
        journal_path, review["approval_sha256"], inventory_sha256, create=not args.observe_only,
    ) as journal:
        finished = execute(members, journal, boundary, dispatch_allowed)
        return {
            "inventory_sha256": inventory_sha256, "completed_members": journal.next_index,
            "outcome": "completed" if finished else "unresolved",
        }


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--application", required=True)
    parser.add_argument("--provider", required=True)
    parser.add_argument("--inventory", required=True, type=Path)
    parser.add_argument("--review", required=True, type=Path)
    parser.add_argument("--state-dir", required=True, type=Path)
    authority = parser.add_mutually_exclusive_group(required=True)
    authority.add_argument("--observe-only", action="store_true")
    authority.add_argument("--policy-tool", type=Path)
    parser.add_argument("--observe-seconds", type=int, default=60, choices=range(1, 301), metavar="SECONDS")
    args = parser.parse_args()
    preflight = runpy.run_path(str(DIRECTORY / "sandbox-retirement-preflight.py"))
    signal.signal(signal.SIGALRM, execution_timeout)
    signal.alarm(900)
    try:
        report = run(args, preflight)
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
