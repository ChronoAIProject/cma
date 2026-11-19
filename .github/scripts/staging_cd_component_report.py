"""Summarize every selected component even when its publication or promotion failed."""
import os
from pathlib import Path

from staging_cd_common import require
from staging_cd_compatibility import GROUP
from staging_cd_component_admission import validate_failures
from staging_cd_deploy_result import STATES
from staging_cd_set import document
from staging_cd_targets import TARGETS

CONCLUSIONS = {"success", "failure", "skipped", "cancelled"}


def outcome(value):
    return value if isinstance(value, str) and value in CONCLUSIONS else "unknown"


def summarize(env):
    changed, allocation = env["CD_PLAN_CHANGED"], env["CD_ALLOCATION_RESULT"]
    require(changed in ("true", "false"), "Plan outcome is missing")
    if changed == "false":
        require(allocation == "skipped", "Unexpected allocation after empty plan")
        return ["No components selected."], True
    if allocation != "success":
        return ["Allocation " + outcome(allocation) + ". No component deployment proven."], False
    lanes = document(env["CD_LANES"])
    require(isinstance(lanes, dict) and set(lanes) <= TARGETS
            and all(lane == c or (lane == "contract" and c in GROUP) for c, lane in lanes.items()),
            "Invalid component routes")
    if not lanes:
        return ["Allocation returned no component work."], True

    passed = env["CD_INFRASTRUCTURE_RESULT"] == "success"
    lines = ["| Component | Publish + finalize | Admit | Deploy job | Controller outcome |",
             "|---|---|---|---|---|"]
    for lane in sorted(set(lanes.values())):
        selected = {c for c in lanes if lanes[c] == lane}
        job = document(env["CD_PROMOTE_" + lane.upper().replace("-", "_")])
        outputs = job.get("outputs", {})
        missing = "skipped" if job["result"] == "skipped" else "unknown"
        admitted = document(outputs.get("admitted") or "[]")
        failures = validate_failures(document(outputs.get("failures") or "{}"))
        require(isinstance(admitted, list) and all(c in TARGETS for c in admitted)
                and len(set(admitted)) == len(admitted) and not set(admitted) & failures.keys(),
                "Invalid admission decision")
        reported = set(admitted) | failures.keys()
        allowed = GROUP if lane == "contract" else selected
        require(reported <= allowed, "Admission escaped component route")
        payload = document(outputs["component_result"]) if outputs.get("component_result") else None
        states = payload["components"] if payload is not None else {}
        require(isinstance(states, dict) and states.keys() <= set(admitted), "Unexpected controller outcome")
        for c in sorted(selected | reported):
            publish = outcome(env["CD_PUBLISH_" + c.upper().replace("-", "_")]) if c in selected else "retained"
            admit = failures.get(c) or ("admitted" if c in admitted else missing)
            deploy = outcome(outputs["deploy"]) if outputs.get("deploy") else missing
            state = "unreported"
            if c in states:
                state = states[c]["state"]
                require(state in STATES and states[c]["reason"] == STATES[state], "Invalid controller outcome")
            lines.append(f"| {c} | {publish} | {admit} | {deploy} | {state} |")
            passed &= (publish in ("success", "retained") and job["result"] == "success"
                       and outputs.get("admit") == "success" and c in admitted and deploy == "success"
                       and state == "controller-verified")
    return lines, passed


def main():
    try:
        lines, passed = summarize(os.environ)
    except (ValueError, TypeError, KeyError):
        lines, passed = ["Component outcome evidence is missing or invalid."], False
    message = "\n".join(lines)
    print(message)
    if os.environ.get("GITHUB_STEP_SUMMARY"):
        with Path(os.environ["GITHUB_STEP_SUMMARY"]).open("a") as stream:
            stream.write(message + "\n")
    return 0 if passed else 1


if __name__ == "__main__":
    raise SystemExit(main())
