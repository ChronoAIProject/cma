"""Owner usage reports and foreign report refusal, using fixed small queries."""

import time

from safety import decode

ISSUES = [1549, 1320, 1333, 1334]
REPORT_SUFFIXES = ("", "/sandboxes?limit=1", "/series?metric=cpu&top=1")


def run(p):
    if "usage_query" not in p.state:
        end = int(time.time()) // 300 * 300 - 300
        p.state["usage_query"] = {"from": end - 600, "to": end,
                                  "step_seconds": 300, "filters": {"statuses": [], "search": ""}}
        p.save()
    for owner, foreign in (("a", "b"), ("b", "a")):
        for name, path in (("summary", "/api/v1/usage"), ("inventory", "/api/v1/usage/sandboxes?limit=20")):
            response = p.request(owner, path)
            p.assert_case("usage_" + name + "_" + owner, response[0] == 200, ISSUES, response)
        label = "report_" + owner
        if label not in p.state["fixtures"]:
            response = p.write(label + "_create", owner, "/api/v1/usage/metrics/reports", "POST", p.state["usage_query"])
            if response[0] in (429, 503):
                p.record(label + "_create", "BLOCKED", ISSUES, response, "report_admission_unavailable_no_retry")
                continue
            p.assert_case(label + "_create", response[0] in (200, 202), ISSUES, response)
            report = decode(response[2])
            p.assert_case(label + "_owner_scope", report.get("scope", {}).get("admin") is False, ISSUES, response, True)
            p.fixture(label, {"report_id": report["report_id"]})
        report_id = p.state["fixtures"][label]["report_id"]
        for suffix in REPORT_SUFFIXES:
            path = "/api/v1/usage/metrics/reports/" + report_id + suffix
            name = "metadata" if not suffix else "rows" if "sandboxes" in suffix else "series"
            response = p.request(foreign, path)
            problem = decode(response[2])
            # The real Mongo adapter conceals another principal's report as
            # NotFound. Capability-denial fixtures exercise a different 403.
            p.assert_case(label + "_" + name + "_foreign_denied", response[0] == 404
                          and problem.get("code") == "not_found", ISSUES, response, True)
            response = p.request(owner, path)
            if response[0] in (202, 429, 503):
                p.record(label + "_" + name + "_owner", "BLOCKED", ISSUES, response, "report_not_ready_no_retry")
                continue
            p.assert_case(label + "_" + name + "_owner", response[0] == 200, ISSUES, response)
    p.record("spend_controls_save_and_enforcement", "BLOCKED", [1489, 1483, 1546],
             reason="ordinary_test_accounts_have_no_platform_authority; shared_policy_mutations_excluded")
