"""Pinned nonstarting admission, original-key recovery, and bounded teardown."""

import time

import launch_recovery
import resources
import trigger_pin
from safety import Refusal, decode

ISSUES = [1536, 1549]
TERMINAL = {"ready", "completed", "failed_retryable", "failed_terminal", "cancelled"}


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



def run(p, new=False):
    trigger = trigger_pin.configured(p)
    response = p.request(None, "/api/v1/triggers/public/" + trigger["public_id"])
    p.assert_case("published_trigger_read", response[0] == 200, ISSUES, response)
    trigger_pin.descriptor(p, decode(response[2]))
    # Recover a lost reply BEFORE checking an inventory that it may have created.
    with p.http.cleanup_budget():
        for actor in ("a", "b"):
            launch_recovery.cleanup(p, actor)
    if new:
        for actor in ("a", "b"):
            launch_recovery.prove_history_and_empty_inventory(p, actor)
        p.state["journey_generation"] = p.state.get("journey_generation", 0) + 1
        p.save()
    generation = p.state.get("journey_generation", 0)
    p.record("j1_always_new", "BLOCKED", [1536],
             reason="approved_trigger_reuses_sandbox; changing_publisher_policy_not_authorized")
    p.record("j2_distinct_workspace", "BLOCKED", [1536],
             reason="approved_trigger_reuses_workspace; changing_publisher_policy_not_authorized")
    for actor, foreign in (("a", "b"), ("b", "a")):
        create_label = "launch_create_" + str(generation) + "_" + actor
        try:
            if create_label not in p.state["writes"]:
                launch_recovery.prove_history_and_empty_inventory(p, actor)
            response = p.write(create_label, actor,
                               "/api/v1/triggers/public/" + trigger["public_id"] + "/launches",
                               "POST", {"start_if_ready": False})
            if response[0] not in (201, 202):
                p.record("journey_admission_" + actor, "BLOCKED", ISSUES, response,
                         "ordinary_visitor_admission_refused_no_privilege_change")
                continue
            launch_recovery.remember(p, actor, decode(response[2]))
            launch = p.state["fixtures"]["launch_" + actor]
            launch_reads(p, actor, foreign, launch)
            path = "/api/v1/launches/" + launch["id"]
            response = p.request(actor, path)
            p.assert_case("launch_current_" + actor, response[0] == 200, ISSUES, response)
            launch = decode(response[2]); trigger_pin.launch(p, launch)
            if launch["state"] == "cancelled":
                p.record("j0_first_answer_" + actor, "BLOCKED", [1536],
                         reason="original_attempt_already_cleaned; new_intent_requires_explicit_new_journey")
                continue
            needs = {need["type"] for need in launch.get("review", {}).get("needs", [])}
            if "connection" in needs:
                p.record("j0_first_answer_" + actor, "BLOCKED", [1536],
                         reason="existing_trigger_requires_unconnected_service; no_connection_or_extra_permission_granted")
                continue
            if needs - {"owner_requires_review"}:
                p.record("j0_first_answer_" + actor, "BLOCKED", [1536],
                         reason="existing_trigger_requires_additional_review_or_choice")
                continue
            if launch["state"] == "draft":
                response = p.write("launch_start_" + str(generation) + "_" + actor, actor,
                                   path + "/start", "POST", {"acknowledgements": []})
                if response[0] not in (200, 202):
                    p.record("journey_start_" + actor, "BLOCKED", ISSUES, response,
                             "ordinary_visitor_start_refused_no_privilege_change")
                    continue
                launch_recovery.remember(p, actor, decode(response[2]))
            for _ in range(24):
                response = p.request(actor, path)
                if response[0] != 200:
                    raise Refusal("launch_poll_unavailable")
                launch = decode(response[2]); launch_recovery.remember(p, actor, launch)
                if launch["state"] in TERMINAL:
                    break
                time.sleep(5)
            if launch["state"] in ("ready", "completed"):
                resources.run_tenant(p, actor, foreign)
            p.record("j0_first_answer_" + actor, "BLOCKED", [1536],
                     reason="trigger_has_no_first_message; complete_answer_not_demonstrated")
        finally:
            with p.http.cleanup_budget():
                launch_recovery.cleanup(p, actor)
