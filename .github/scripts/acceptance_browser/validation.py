"""Reject arbitrary browser output; never pass unknown strings into evidence."""

import re

from safety import Refusal

PHASES = {"A1", "A2", "A3", "A4", "A5_focus", "A5_return", "A6", "A7"}
ROUTES = {
    "AUTH",
    "BUILD",
    "INVENTORY",
    "TRANSPORT",
    "DEFAULTS",
    "SANDBOX",
    "WORKSPACE",
    "WORKTREE",
    "AGENT",
    "HISTORY",
    "USAGE",
    "SOURCE",
    "SETTINGS",
    "ACCESS",
    "HEAD",
    "SNAPSHOT",
    "STREAM",
    "QUEUE",
    "RESPONSE",
    "COMMAND",
    "AGUI",
    "PAGE",
    "ASSET",
    "STATIC",
    "OTHER",
}


def require(condition):
    if not condition:
        raise Refusal("browser_output_schema_refused")


def keys(value, expected):
    require(isinstance(value, dict) and set(value) == set(expected.split()))


def number(value, maximum=1800000):
    require(type(value) is int and 0 <= value <= maximum)


def fingerprint(value):
    require(isinstance(value, str) and re.fullmatch(r"[a-f0-9]{64}", value) is not None)


def validate(value):
    keys(
        value,
        "requests script_errors blocked cases initialized actual_hidden navigation_return "
        "readiness_ms history continuity browser_version observation_complete",
    )
    require(re.fullmatch(r"\d+\.\d+\.\d+\.\d+", value["browser_version"]) is not None)
    for key in (
        "initialized",
        "actual_hidden",
        "navigation_return",
        "observation_complete",
    ):
        require(type(value[key]) is bool)
    number(value["script_errors"])
    if value["readiness_ms"] is not None:
        number(value["readiness_ms"])
    for key in ("history", "continuity"):
        content = value[key]
        if content is not None:
            keys(content, "history_count history_sha256 draft_sha256 composer_ready")
            number(content["history_count"])
            fingerprint(content["history_sha256"])
            fingerprint(content["draft_sha256"])
            require(type(content["composer_ready"]) is bool)
    keys(value["blocked"], "WRITE DESTINATION SOCKET BUDGET QUIET STOPPED")
    for count in value["blocked"].values():
        number(count)
    require(isinstance(value["cases"], list) and len(value["cases"]) <= 8)
    for case in value["cases"]:
        keys(case, "phase start_ms window_start_ms end_ms")
        require(case["phase"] in PHASES)
        for field in ("start_ms", "window_start_ms", "end_ms"):
            number(case[field])
        require(case["start_ms"] <= case["window_start_ms"] <= case["end_ms"])
    require(isinstance(value["requests"], list) and len(value["requests"]) <= 1600)
    for row in value["requests"]:
        keys(
            row,
            "phase route method start_ms end_ms status canceled censored hidden_start duplicate sse correlation_sha256",
        )
        require(row["phase"] in PHASES | {"UNMEASURED"} and row["route"] in ROUTES)
        require(row["method"] in ("GET", "HEAD", "OPTIONS", "WRITE"))
        number(row["start_ms"])
        if row["end_ms"] is not None:
            number(row["end_ms"])
            require(row["end_ms"] >= row["start_ms"])
        if row["status"] is not None:
            number(row["status"], 599)
            require(row["status"] >= 100)
        for field in ("canceled", "censored", "hidden_start", "duplicate", "sse"):
            require(type(row[field]) is bool)
        if row["correlation_sha256"] is not None:
            fingerprint(row["correlation_sha256"])
    return value


def assess(browser, authenticated):
    cases = [
        {"case": f"A{i}", "result": "NOT OBSERVED", "reason": "fixture_unavailable"}
        for i in range(1, 8)
    ]
    cases[0]["reason"] = "controller_release_receipt_required"
    if not authenticated:
        if not browser["initialized"] or browser["script_errors"]:
            cases[0].update(result="FAIL", reason="public_initialization_failed")
        for case in cases[1:]:
            case["reason"] = "authenticated_fixture_required"
        return cases
    phases = {entry["phase"] for entry in browser["cases"]}
    invalid = any(browser["blocked"].values()) or not browser["observation_complete"]
    for case in cases[1:5]:
        phase = case["case"]
        expected = {"A5_focus", "A5_return"} if phase == "A5" else {phase}
        if not expected <= phases:
            case["reason"] = "phase_incomplete"
            continue
        rows = [row for row in browser["requests"] if row["phase"] in expected]
        good = (
            not invalid
            and browser["script_errors"] == 0
            and not any(
                row["duplicate"] or (row["status"] is not None and row["status"] >= 400)
                for row in rows
            )
        )
        if phase == "A2":
            good = good and bool(
                browser["history"]
                and browser["history"]["history_count"]
                and browser["history"]["composer_ready"]
            )
            good = (
                good and sum(row["sse"] and row["status"] == 200 for row in rows) == 1
            )
        if phase == "A4":
            good = (
                good
                and browser["actual_hidden"]
                and not any(
                    row["hidden_start"]
                    and row["route"] in {"SANDBOX", "AGENT", "HEAD", "QUEUE"}
                    for row in rows
                )
            )
        if phase == "A5":
            good = (
                good
                and browser["navigation_return"]
                and browser["history"] == browser["continuity"]
            )
        case.update(
            result="PASS" if good else "FAIL",
            reason="observed_readonly_behavior"
            if good
            else "observation_requires_review",
        )
    return cases
