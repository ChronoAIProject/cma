"""Recover original Launch receipts before inventory or fresh admission."""

import time

from identity import fingerprint
import launch_ledger
import trigger_pin
from safety import Refusal, decode

ISSUES = [1536, 1549]


def remember(p, actor, launch):
    trigger_pin.launch(p, launch)
    p.fixture("launch_" + actor, launch)
    sandbox_id = launch.get("resolution", {}).get("sandbox", {}).get("id")
    if not sandbox_id:
        return
    response = p.request(actor, "/api/v1/sandboxes/" + sandbox_id)
    if response[0] == 404:
        # Only cleanup may interpret absence; never start using this observation.
        return
    p.assert_case("journey_sandbox_owner_read_" + actor, response[0] == 200, ISSUES, response)
    detail = decode(response[2]); sandbox = detail.get("sandbox", detail)
    p.assert_case("journey_sandbox_owner_equal_" + actor,
                  fingerprint(sandbox.get("owner", "")) == p.approval["actors"][actor]["subject_sha256"],
                  ISSUES, security=True)
    old = p.state["fixtures"].get("sandbox_" + actor, {})
    if old.get("id") and old["id"] != sandbox_id and not p.state.get("cleaned_sandboxes", {}).get(actor):
        raise Refusal("second_live_visitor_sandbox_refused")
    if old.get("id") != sandbox_id:
        p.state.setdefault("cleaned_sandboxes", {}).pop(actor, None)
    p.fixture("sandbox_" + actor, sandbox)


def recover(p, actor):
    recovered = []
    for entry in launch_ledger.pending(p, actor):
        if entry.get("launch_id"):
            launch_id = entry["launch_id"]
        else:
            intent = entry["intent"]
            launch_ledger.validate(p, actor, entry["key"], intent)
            launch_ledger.submitted(p, entry["key"])
            response = p.request(actor, intent["path"], intent["method"], intent["body"], key=entry["key"])
            body = decode(response[2])
            launch_ledger.observed(p, entry["key"], response[0], body)
            updated = launch_ledger.load(p)["entries"][entry["key"]]
            if updated["status"] == "refused":
                continue
            if updated["status"] != "resolved":
                raise Refusal("original_launch_receipt_still_uncertain")
            launch_id = updated["launch_id"]
        response = p.request(actor, "/api/v1/launches/" + launch_id)
        if response[0] != 200:
            raise Refusal("recovered_launch_owner_read_unavailable")
        launch = decode(response[2])
        trigger_pin.launch(p, launch)
        recovered.append(launch)
    return recovered


def cleanup(p, actor):
    recovered = recover(p, actor)
    known = p.state["fixtures"].get("launch_" + actor)
    if known and all(item["id"] != known["id"] for item in recovered):
        recovered.append(known)
    for launch in recovered:
        path = "/api/v1/launches/" + launch["id"]
        response = p.request(actor, path)
        if response[0] != 200:
            raise Refusal("cleanup_launch_read_unavailable")
        current = decode(response[2]); remember(p, actor, current)
        if current["state"] not in ("ready", "completed", "cancelled"):
            label = "launch_cancel_" + fingerprint(launch["id"])
            response = p.write(label, actor, path + "/cancel", "POST", None)
            p.assert_case("launch_cancel_" + actor, response[0] == 200, ISSUES, response)
            response = p.request(actor, path)
            if response[0] != 200:
                raise Refusal("cancelled_launch_reconciliation_required")
            current = decode(response[2]); remember(p, actor, current)
        if current["state"] not in ("ready", "completed", "cancelled"):
            raise Refusal("launch_cleanup_requires_confirmed_terminal_state")
        sandbox_id = current.get("resolution", {}).get("sandbox", {}).get("id")
        if sandbox_id:
            remove_sandbox(p, actor, sandbox_id)
        launch_ledger.cleaned(p, actor, launch["id"])
    response = p.request(actor, "/api/v1/sandboxes?exclude_retired=true&limit=2")
    page = decode(response[2])
    p.assert_case("journey_no_sandbox_left_" + actor,
                  response[0] == 200 and not page.get("items") and not page.get("next_cursor"), ISSUES, response)


def remove_sandbox(p, actor, sandbox_id):
    path = "/api/v1/sandboxes/" + sandbox_id
    label = "sandbox_delete_" + fingerprint(sandbox_id)
    for attempt in range(24):
        response = p.request(actor, path)
        if response[0] == 404 or (response[0] == 200 and decode(response[2]).get("sandbox", decode(response[2])).get("retired_at")):
            p.state.setdefault("cleaned_sandboxes", {})[actor] = True
            p.save()
            p.record("journey_sandbox_removed_" + actor, "PASS", ISSUES, response,
                     "sandbox_sha256_" + fingerprint(sandbox_id))
            return
        if response[0] != 200:
            raise Refusal("cleanup_sandbox_read_unavailable")
        detail = decode(response[2]); sandbox = detail.get("sandbox", detail)
        if fingerprint(sandbox.get("owner", "")) != p.approval["actors"][actor]["subject_sha256"]:
            raise Refusal("cleanup_sandbox_owner_mismatch")
        p.fixture("sandbox_" + actor, sandbox)
        if attempt == 0:
            response = p.write(label, actor, path, "DELETE", None)
            p.assert_case("journey_sandbox_delete_" + actor, response[0] == 202, ISSUES, response)
        time.sleep(5)
    raise Refusal("sandbox_cleanup_pending_supported_delete_accepted")


def prove_history_and_empty_inventory(p, actor):
    # No new key while any earlier key is uncertain or has resources to clean.
    if launch_ledger.pending(p, actor):
        raise Refusal("pending_launch_requires_cleanup")
    response = p.request(actor, "/api/v1/launches?limit=100")
    if response[0] != 200:
        raise Refusal("server_launch_history_unavailable")
    page = decode(response[2])
    known = {e["launch_id"] for e in launch_ledger.load(p)["entries"].values()
             if e["actor"] == actor and e.get("launch_id")}
    if page.get("next_cursor") or {r["id"] for r in page.get("items", [])} != known:
        raise Refusal("server_launch_history_differs_from_durable_account_ledger")
    response = p.request(actor, "/api/v1/sandboxes?exclude_retired=true&limit=2")
    if response[0] != 200 or decode(response[2]).get("items"):
        raise Refusal("new_launch_requires_empty_owned_sandbox_inventory")
