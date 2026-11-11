"""Existing published Trigger only; no elevated grants or publisher mutations."""

import hashlib
import time

import resources
from journey_scope import reserve_launch
from safety import Refusal, decode, read_private

ISSUES = [1536, 1549]
TERMINAL = {"ready", "completed", "failed_retryable", "failed_terminal", "cancelled"}


def remember(p, tenant, launch):
    p.fixture("launch_" + tenant, launch)
    sandbox_id = launch.get("resolution", {}).get("sandbox", {}).get("id")
    if sandbox_id:
        response = p.request(tenant, "/api/v1/sandboxes/" + sandbox_id)
        p.assert_case("journey_sandbox_owner_read_" + tenant, response[0] == 200,
                      ISSUES, response)
        detail = decode(response[2])
        sandbox = detail.get("sandbox", detail)
        p.assert_case("journey_sandbox_owner_equal_" + tenant,
                      sandbox.get("owner") == p.accounts[tenant]["subject"],
                      ISSUES, security=True)
        old = p.state["fixtures"].get("sandbox_" + tenant, {})
        if old.get("id") and old["id"] != sandbox_id:
            raise Refusal("one_visitor_sandbox_limit")
        p.fixture("sandbox_" + tenant, sandbox)


def launch_reads(p, tenant, foreign, launch):
    path = "/api/v1/launches/" + launch["id"]
    for label, suffix in (("metadata", ""), ("events", "/events?after=0&limit=20"),
                          ("requirements", "/requirements")):
        response = p.request(foreign, path + suffix)
        p.assert_case("launch_" + label + "_foreign_denied_" + tenant,
                      response[0] == 404 and decode(response[2]).get("code") == "launch_not_found",
                      ISSUES, response, True)
        response = p.request(tenant, path + suffix)
        p.assert_case("launch_" + label + "_owner_" + tenant, response[0] == 200,
                      ISSUES, response)
    for caller in (tenant, foreign):
        response = p.request(caller, "/api/v1/launches?limit=20")
        rows = decode(response[2]).get("items", [])
        present = any(row.get("id") == launch["id"] for row in rows)
        p.assert_case("launch_list_" + tenant + "_caller_" + caller,
                      response[0] == 200 and present == (caller == tenant), ISSUES, response, True)


def cleanup(p, tenant):
    launch = p.state["fixtures"].get("launch_" + tenant)
    if launch:
        path = "/api/v1/launches/" + launch["id"]
        response = p.request(tenant, path)
        if response[0] != 200:
            raise Refusal("cleanup_launch_read_unavailable")
        current = decode(response[2])
        remember(p, tenant, current)
        if current["state"] not in ("ready", "completed", "cancelled"):
            response = p.write("launch_cancel_" + tenant, tenant, path + "/cancel", "POST", None)
            p.assert_case("launch_cancel_" + tenant, response[0] == 200,
                          ISSUES, response)
            remember(p, tenant, decode(response[2]))
    sandbox = p.state["fixtures"].get("sandbox_" + tenant)
    if not sandbox:
        response = p.request(tenant, "/api/v1/sandboxes?exclude_retired=true&limit=2")
        p.assert_case("journey_no_sandbox_left_" + tenant,
                      response[0] == 200 and not decode(response[2]).get("items"), ISSUES, response)
        return
    path = "/api/v1/sandboxes/" + sandbox["id"]
    response = p.write("sandbox_delete_" + tenant, tenant, path, "DELETE", None)
    p.assert_case("journey_sandbox_delete_" + tenant, response[0] == 202, ISSUES, response)
    for _ in range(24):
        response = p.request(tenant, path)
        if response[0] == 404:
            p.record("journey_sandbox_removed_" + tenant, "PASS", ISSUES, response,
                     "sandbox_sha256_" + hashlib.sha256(sandbox["id"].encode()).hexdigest())
            p.state.setdefault("cleaned_sandboxes", {})[tenant] = True
            p.save()
            return
        if response[0] == 200:
            detail = decode(response[2])
            if detail.get("sandbox", detail).get("retired_at"):
                p.record("journey_sandbox_retired_" + tenant, "PASS", ISSUES, response,
                         "sandbox_sha256_" + hashlib.sha256(sandbox["id"].encode()).hexdigest())
                p.state.setdefault("cleaned_sandboxes", {})[tenant] = True
                p.save()
                return
        time.sleep(5)
    raise Refusal("sandbox_cleanup_pending_supported_delete_accepted")


def run(p):
    trigger = read_private("acceptance-trigger.json")
    if not trigger:
        raise Refusal("approved_existing_trigger_required")
    response = p.request(None, "/api/v1/triggers/public/" + trigger["public_id"])
    p.assert_case("published_trigger_read", response[0] == 200, ISSUES, response)
    descriptor = decode(response[2])
    if descriptor != trigger["public_descriptor"]:
        raise Refusal("approved_trigger_descriptor_changed")
    if descriptor.get("availability") != "published" or descriptor.get("trust", {}).get("disclosures"):
        raise Refusal("trigger_unavailable_or_requires_software_trust")
    p.record("j1_always_new", "BLOCKED", [1536],
             reason="approved_trigger_reuses_sandbox; changing_publisher_policy_not_authorized")
    p.record("j2_distinct_workspace", "BLOCKED", [1536],
             reason="approved_trigger_reuses_workspace; changing_publisher_policy_not_authorized")
    for tenant, foreign in (("a", "b"), ("b", "a")):
        label = "launch_" + tenant
        create_label = "launch_create_" + tenant
        previous = p.state["writes"].get(create_label, {})
        if (p.accounts[tenant].get("cma_consent_scopes") and label not in p.state["fixtures"]
                and previous.get("response_status") == 409
                and previous.get("response", {}).get("code") == "sandbox_credential_setup_required"
                and create_label + "_before_consent" not in p.state["writes"]):
            # The public problem explicitly requires a new key after sign-in.
            # Preserve the refused receipt and count this as another attempt.
            p.state["writes"][create_label + "_before_consent"] = previous
            del p.state["writes"][create_label]
            p.state["launch_reservations"].pop(tenant, None)
            p.save()
        try:
            if label not in p.state["fixtures"]:
                response = p.request(tenant, "/api/v1/sandboxes?exclude_retired=true&limit=2")
                if response[0] != 200 or decode(response[2]).get("items"):
                    raise Refusal("new_launch_requires_empty_owned_sandbox_inventory")
                reserve_launch(p, tenant)
                response = p.write("launch_create_" + tenant, tenant,
                                   "/api/v1/triggers/public/" + trigger["public_id"] + "/launches",
                                   "POST", {"start_if_ready": True})
                if response[0] in (403, 409, 429, 503):
                    p.record("journey_admission_" + tenant, "BLOCKED", ISSUES, response,
                             "ordinary_visitor_admission_refused_no_privilege_change")
                    continue
                p.assert_case("journey_admission_" + tenant, response[0] in (201, 202), ISSUES, response)
                remember(p, tenant, decode(response[2]))
            launch = p.state["fixtures"][label]
            launch_reads(p, tenant, foreign, launch)
            path = "/api/v1/launches/" + launch["id"]
            response = p.request(tenant, path)
            p.assert_case("launch_current_" + tenant, response[0] == 200, ISSUES, response)
            launch = decode(response[2])
            needs = {need["type"] for need in launch.get("review", {}).get("needs", [])}
            if "connection" in needs:
                p.record("j0_first_answer_" + tenant, "BLOCKED", [1536],
                         reason="existing_trigger_requires_unconnected_service; no_connection_or_extra_permission_granted")
                continue
            if needs - {"owner_requires_review"}:
                p.record("j0_first_answer_" + tenant, "BLOCKED", [1536],
                         reason="existing_trigger_requires_additional_review_or_choice")
                continue
            if launch["state"] == "draft":
                response = p.write("launch_start_" + tenant, tenant, path + "/start", "POST", {"acknowledgements": []})
                if response[0] not in (200, 202):
                    p.record("journey_start_" + tenant, "BLOCKED", ISSUES, response,
                             "ordinary_visitor_start_refused_no_privilege_change")
                    continue
                remember(p, tenant, decode(response[2]))
            for _ in range(24):
                response = p.request(tenant, path)
                if response[0] != 200:
                    raise Refusal("launch_poll_unavailable")
                launch = decode(response[2])
                remember(p, tenant, launch)
                if launch["state"] in TERMINAL:
                    break
                time.sleep(5)
            if launch["state"] in ("ready", "completed"):
                resources.run_tenant(p, tenant, foreign)
            p.record("j0_first_answer_" + tenant, "BLOCKED", [1536],
                     reason="trigger_has_no_first_message; complete_answer_not_demonstrated")
        finally:
            cleanup(p, tenant)
