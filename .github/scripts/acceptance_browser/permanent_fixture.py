"""Lead #215: one permanent ordinary-user Agent, never replace or delete it."""

import argparse
import fcntl
import json
import os
from pathlib import Path
import re
import sys
import uuid

sys.path.insert(0, str(Path(__file__).resolve().parent.parent / "acceptance"))
from evidence import Evidence
from identity import canonical_hash, fingerprint
from probe import Probe
from safety import (
    CMA_ORIGIN,
    Refusal,
    decode,
    outside_quiet_window,
    private_root,
    read_private,
    write_private,
)
from binding import bind_browser

NAME = "web-verification-permanent-agent.json"
BODY = {
    "name": "CMA automated acceptance fixture",
    "machine_included": False,
    "desktop": False,
    "profile": {
        "reasoning_effort": "minimal",
        "permission_mode": "guarded",
        "skills": [],
        "permissions": {"services": []},
    },
}


def load(p):
    entry = read_private(NAME)
    if entry and (
        entry.get("version") != 1
        or entry.get("actor") != "a"
        or entry.get("subject_sha256") != p.approval["actors"]["a"]["subject_sha256"]
        or entry.get("intent_sha256") != canonical_hash(BODY)
        or entry.get("intent") != BODY
        or entry.get("permanent") is not True
    ):
        raise Refusal("permanent_fixture_binding_mismatch")
    return entry


def prepare(p):
    existing = load(p)
    if existing:
        return existing
    for actor in ("a", "b"):
        for path in (
            "/api/v1/sandboxes?exclude_retired=true&limit=100",
            "/api/v1/agents/provenance?limit=100",
        ):
            response = p.request(actor, path)
            value = decode(response[2])
            if response[0] != 200 or not isinstance(value.get("items"), list):
                raise Refusal("complete_owned_inventory_required")
            if value["items"] or value.get("next_cursor"):
                raise Refusal("existing_fixture_requires_reuse_not_creation")
    entry = {
        "version": 1,
        "actor": "a",
        "subject_sha256": p.approval["actors"]["a"]["subject_sha256"],
        "intent_sha256": canonical_hash(BODY),
        "intent": BODY,
        "key": str(uuid.uuid4()),
        "status": "prepared",
        "permanent": True,
        "idle_required": True,
    }
    write_private(NAME, entry)
    return entry


def admit(p, entry):
    if entry["status"] != "prepared":
        if not entry.get("operation_id"):
            raise Refusal("original_create_receipt_reconciliation_required")
        return
    entry["status"] = "submitted"
    write_private(NAME, entry)
    response = p.http.request(
        CMA_ORIGIN,
        "/api/agent-manager/v1/agents",
        "POST",
        token=p.accounts["a"]["access_token"],
        body=entry["intent"],
        headers={"Idempotency-Key": entry["key"]},
    )
    payload = decode(response[2])
    entry.update(response_status=response[0], receipt=payload)
    if response[0] == 202 and re.fullmatch(r"[A-Za-z0-9_-]+", payload.get("id", "")):
        entry.update(status="admitted", operation_id=payload["id"])
    write_private(NAME, entry)


def observe(p, entry):
    if not entry.get("operation_id"):
        return {
            "status": "BLOCKED",
            "reason": "create_not_admitted",
            "http_status": entry.get("response_status"),
        }
    response = p.request(
        "a", "/api/agent-manager/v1/agent-lifecycle/" + entry["operation_id"]
    )
    if response[0] != 200:
        raise Refusal("lifecycle_read_unavailable")
    value = decode(response[2])
    entry["operation"] = value
    state = value.get("status")
    allowed = {"queued", "running", "waiting_for_connection", "succeeded", "failed"}
    if state not in allowed:
        raise Refusal("lifecycle_status_unrecognized")
    entry["status"] = state
    if value.get("agent_id"):
        if not re.fullmatch(r"[A-Za-z0-9_-]+", value["agent_id"]):
            raise Refusal("agent_identity_invalid")
        entry["agent_id"] = value["agent_id"]
        detail = p.request("a", "/api/v1/agents/" + entry["agent_id"])
        if detail[0] == 200:
            agent = decode(detail[2])
            sandbox = agent.get("sandbox", {}).get("id", "")
            if not re.fullmatch(r"[A-Za-z0-9_-]+", sandbox):
                raise Refusal("agent_parent_invalid")
            parent = p.request("a", "/api/v1/sandboxes/" + sandbox)
            owner = decode(parent[2])
            owner = owner.get("sandbox", owner)
            if (
                parent[0] != 200
                or fingerprint(owner.get("owner", "")) != entry["subject_sha256"]
            ):
                raise Refusal("fixture_owner_mismatch")
            entry.update(
                sandbox_id=sandbox,
                agent_status=agent.get("status"),
                sandbox_status=owner.get("status"),
            )
    write_private(NAME, entry)
    return {
        "status": state,
        "agent_present": bool(entry.get("agent_id")),
        "agent_sha256": fingerprint(entry["agent_id"])
        if entry.get("agent_id")
        else None,
        "pause_obligation": entry["idle_required"],
        "required_connection_count": len(
            value.get("connection", {}).get("services", [])
        )
        if value.get("connection")
        else 0,
    }


def idle(p, entry):
    sandbox = entry.get("sandbox_id")
    if not sandbox:
        # A failed provisioning operation may have allocated a parent before the
        # Agent exists. Only one owned parent can be attributed to the empty census.
        response = p.request("a", "/api/v1/sandboxes?exclude_retired=true&limit=2")
        data = decode(response[2])
        if (
            response[0] != 200
            or data.get("next_cursor")
            or not isinstance(data.get("items"), list)
        ):
            raise Refusal("idle_parent_inventory_unavailable")
        if len(data["items"]) == 0:
            return {"status": "NO_PARENT", "pause_obligation": entry["idle_required"]}
        if len(data["items"]) != 1:
            raise Refusal("idle_parent_ambiguous")
        item = data["items"][0]
        item = item.get("sandbox", item)
        sandbox = item.get("id")
    if not isinstance(sandbox, str) or not re.fullmatch(r"[A-Za-z0-9_-]+", sandbox):
        raise Refusal("idle_parent_invalid")
    response = p.request("a", "/api/v1/sandboxes/" + sandbox)
    owner = decode(response[2])
    owner = owner.get("sandbox", owner)
    if (
        response[0] != 200
        or fingerprint(owner.get("owner", "")) != entry["subject_sha256"]
    ):
        raise Refusal("idle_owner_mismatch")
    entry["sandbox_id"] = sandbox
    state = owner.get("status")
    if state == "paused":
        entry.update(idle_required=False, sandbox_status="paused")
        write_private(NAME, entry)
        return {
            "status": "PAUSED",
            "sandbox_sha256": fingerprint(sandbox),
            "pause_obligation": False,
        }
    entry.setdefault("pause_key", str(uuid.uuid4()))
    entry["pause_status"] = "submitted"
    write_private(NAME, entry)
    response = p.http.request(
        CMA_ORIGIN,
        "/api/v1/sandboxes/" + sandbox + "/pause",
        "POST",
        token=p.accounts["a"]["access_token"],
        body={},
        headers={"Idempotency-Key": entry["pause_key"]},
    )
    entry["pause_http_status"] = response[0]
    write_private(NAME, entry)
    return {
        "status": "PAUSE_PENDING",
        "http_status": response[0],
        "pause_obligation": True,
    }


def run(mode, output):
    outside_quiet_window()
    with Evidence(output) as evidence:
        fd = os.open(
            private_root() / "runner.lock",
            os.O_CREAT | os.O_RDWR | os.O_NOFOLLOW,
            0o600,
        )
        with os.fdopen(fd, "w") as lock:
            fcntl.flock(lock, fcntl.LOCK_EX | fcntl.LOCK_NB)
            p = Probe()
            bind_browser(p)
            p.http.writes_authorized = True
            entry = prepare(p) if mode == "create" else load(p)
            if not entry:
                raise Refusal("permanent_fixture_receipt_required")
            if mode == "create":
                admit(p, entry)
            result = idle(p, entry) if mode == "idle" else observe(p, entry)
            evidence.publish(result)
            print(json.dumps(result))


if __name__ == "__main__":
    parser = argparse.ArgumentParser()
    parser.add_argument("mode", choices=("create", "observe", "idle"))
    parser.add_argument("--output", required=True)
    args = parser.parse_args()
    try:
        run(args.mode, args.output)
    except Exception:
        print(
            "FIXTURE BLOCKED: original private receipt requires reconciliation; no replacement"
        )
        sys.exit(1)
