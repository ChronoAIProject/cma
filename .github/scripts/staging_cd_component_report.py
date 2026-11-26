"""Summarize every selected component even when its publication or promotion failed."""
import os
from pathlib import Path

from ci_outcome import cell, entry
from staging_cd_common import require
from staging_cd_compatibility import GROUP
from staging_cd_component_admission import REASONS as ADMISSION_REASONS, validate_failures
from staging_cd_deploy_result import STATES
from staging_cd_set import ALREADY_VERIFIED, OVERTAKEN, RETRY_PENDING, document
from staging_cd_targets import TARGETS

CONCLUSIONS = {"success", "failure", "skipped", "cancelled"}
# A newer reservation's skip is handled: that newer run deploys. When an older
# run finalized the same source first, no run is known to own the deployment
# (a newer run may have planned around this one), so that skip always fails.
DISPOSITIONS = frozenset({"reservation_superseded", "source_already_final"})
UNSUCCESSFUL = ("failure", "skipped", "cancelled", "unknown")
# Every code a row or a run-level line can explain, from the component-report catalogue family.
CODES = frozenset({"allocation-incomplete", "evidence-invalid", *DISPOSITIONS, OVERTAKEN, "superseded-peer",
                   "finalize-incomplete", "admit-skipped", "admit-unknown", "controller-unreported", "retry-not-run",
                   *("publish-" + value for value in UNSUCCESSFUL), *("deploy-" + value for value in UNSUCCESSFUL)})
HEADER = ["| Component | Publish + finalize | Admit | Deploy job | Controller outcome | Reason | Likely cause | Next step |",
          "|---|---|---|---|---|---|---|---|"]


def outcome(value):
    return value if isinstance(value, str) and value in CONCLUSIONS else "unknown"


def diagnosis(publish, admit, deploy, state):
    """Return (family, code) for a row's first unproven column, or None if verified."""
    if publish in DISPOSITIONS or publish in ("superseded-peer", "finalize-incomplete"):
        return "component-report", publish
    if publish not in ("success", "retained"):
        return "component-report", "publish-" + publish
    if admit in ADMISSION_REASONS:
        return "admission", admit
    if admit != "admitted":
        return "component-report", "admit-" + admit
    if state == ALREADY_VERIFIED:
        return None
    if deploy == RETRY_PENDING:
        return "component-report", "retry-not-run"
    if deploy != "success":
        return "component-report", "deploy-" + deploy
    if state in ("withheld", "unverified"):
        return "deploy-component", STATES[state]
    if state == OVERTAKEN:
        return "component-report", OVERTAKEN
    if state != "controller-verified":
        return "component-report", "controller-unreported"
    return None


def row(component, publish, admit, deploy, state, *, reason=None):
    """One table row; the reason columns explain the first column that is not proven."""
    found = diagnosis(reason or publish, admit, deploy, state)
    if found is None:
        explained = "none | - | -"
    else:
        value = entry(*found)
        explained = f"{found[1]} | {value['cause']} | {cell(value['next'])}"
    return f"| {component} | {publish} | {admit} | {deploy} | {state} | {explained} |"


def next_step(code):
    return "Next step: " + entry("component-report", code)["next"]


def summarize(env):
    """True requires verified releases; None denotes a handled takeover."""
    changed, allocation = env["CD_PLAN_CHANGED"], env["CD_ALLOCATION_RESULT"]
    require(changed in ("true", "false"), "Plan outcome is missing")
    if changed == "false":
        require(allocation == "skipped", "Unexpected allocation after empty plan")
        return ["No components selected."], True
    if allocation != "success":
        return ["Allocation " + outcome(allocation) + ". No component deployment proven.",
                next_step("allocation-incomplete")], False
    lanes = document(env["CD_LANES"])
    require(isinstance(lanes, dict) and set(lanes) <= TARGETS
            and all(lane == c or (lane == "contract" and c in GROUP) for c, lane in lanes.items()),
            "Invalid component routes")
    if not lanes:
        return ["Allocation returned no component work."], True

    superseded = {}
    for c in sorted(TARGETS):
        disposition = env.get("CD_DISPOSITION_" + c.upper().replace("-", "_"), "")
        require(disposition == "" or disposition in DISPOSITIONS, "Unknown publication disposition")
        if disposition:
            require(c in lanes and not env.get("CD_FINALIZED_" + c.upper().replace("-", "_")),
                    "Superseded publication conflicts with finalization evidence")
            superseded[c] = disposition
    overtaken, admission_overtaken, already_verified = set(), set(), set()
    passed = env["CD_INFRASTRUCTURE_RESULT"] == "success"
    lines = list(HEADER)
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
        disposition = outputs.get("deploy_disposition") or ""
        require(disposition in ("", OVERTAKEN, RETRY_PENDING), "Unknown deployment disposition")
        if outputs.get("retried") == "true":
            # The promotion's outputs are its automatic retry's own attempt.
            lines.append(f"{lane}: the first deployment was refused for a transient reason before any "
                         "change; the rows below are its automatic re-admission and retry.")
        if disposition == RETRY_PENDING:
            # The first attempt handed over to one re-admission that never reported.
            require(outputs.get("retried") != "true", "Retry outcome conflicts with its evidence")
            for c in sorted(selected | reported):
                publish = outcome(env["CD_PUBLISH_" + c.upper().replace("-", "_")]) if c in selected else "retained"
                admit = failures.get(c) or ("admitted" if c in admitted else missing)
                lines.append(row(c, publish, admit, RETRY_PENDING, "unreported"))
            passed = False
            continue
        if disposition:
            # The deploy job found a newer final at a later source for every
            # admitted member and deployed nothing. Its run owns the release.
            require(lane != "contract" and bool(admitted) and not GROUP & set(admitted) and payload is None
                    and outputs.get("deploy") == "success", "Overtaken deployment conflicts with its evidence")
            overtaken.update(admitted)
        admission = outputs.get("admit_disposition") or ""
        require(admission in ("", OVERTAKEN, ALREADY_VERIFIED), "Unknown admission disposition")
        if admission == ALREADY_VERIFIED:
            require(selected <= set(admitted) and not failures and not disposition and payload is None
                    and job["result"] == "success" and outputs.get("admit") == "success" and not outputs.get("deploy"),
                    "Reused controller proof conflicts with admission evidence")
            already_verified.update(admitted)
        elif admission == OVERTAKEN:
            # Admission found a later-source final for every selected member.
            require(lane != "contract" and not GROUP & selected and not admitted and not disposition
                    and failures.keys() == selected and set(failures.values()) == {"superseded-source"}
                    and outputs.get("admit") == "success" and not outputs.get("deploy") and payload is None,
                    "Overtaken admission conflicts with its evidence")
            admission_overtaken.update(selected)
        if superseded.keys() & selected:
            # A contract cannot promote partially. Check the other selected
            # publishers independently so supersession cannot conceal a failure.
            passed &= job["result"] == "skipped" and not any(outputs.values())
            for c in sorted(selected):
                suffix = c.upper().replace("-", "_")
                result = outcome(env["CD_PUBLISH_" + suffix])
                passed &= result == "success"
                reason = None
                if c in superseded and result == "success":
                    result = superseded[c]
                elif c not in superseded:
                    finalized = env.get("CD_FINALIZED_" + suffix) == "success"
                    passed &= finalized
                    if result == "success":
                        reason = "superseded-peer" if finalized else "finalize-incomplete"
                lines.append(row(c, result, missing, missing, "unreported", reason=reason))
            continue
        for c in sorted(selected | reported):
            publish = outcome(env["CD_PUBLISH_" + c.upper().replace("-", "_")]) if c in selected else "retained"
            admit = failures.get(c) or ("admitted" if c in admitted else missing)
            # A handled admission deliberately ran no deployment job.
            confirmed = admission == ALREADY_VERIFIED and c in admitted
            handled = c in admission_overtaken or confirmed
            deploy = outcome(outputs["deploy"]) if outputs.get("deploy") else "skipped" if handled else missing
            state = ALREADY_VERIFIED if confirmed else (
                OVERTAKEN if c in overtaken | admission_overtaken else "unreported")
            if c in states:
                state = states[c]["state"]
                require(state in STATES and states[c]["reason"] == STATES[state], "Invalid controller outcome")
            lines.append(row(c, publish, admit, deploy, state))
            passed &= (publish in ("success", "retained") and job["result"] == "success"
                       and outputs.get("admit") == "success"
                       and (handled or (c in admitted and deploy == "success"
                                        and state in ("controller-verified", OVERTAKEN))))
    for c, disposition in sorted(superseded.items()):
        lines.append(f"{c} publication skipped: {disposition}. This run has no verified {c} release.")
        if disposition == "source_already_final":
            lines.append(f"{c}: an older run finalized this source. The next develop push resumes this "
                         "group's final; push to deploy (a re-run allocates nothing once develop moves on).")
            passed = False
    for c in sorted(overtaken):
        lines.append(f"{c} deployment skipped: {OVERTAKEN}. A newer final's run owns this deployment; "
                     f"this run has no verified {c} release.")
    for c in sorted(admission_overtaken):
        lines.append(f"{c} admission skipped: {OVERTAKEN}. A newer final's run owns this deployment; "
                     f"this run has no verified {c} release.")
    for c in sorted(already_verified):
        lines.append(f"{c} deployment skipped: a previous staging deployment verified this exact release.")
    if (superseded or overtaken or admission_overtaken) and passed:
        return lines, None
    return lines, passed


def main():
    try:
        lines, passed = summarize(os.environ)
    except (ValueError, TypeError, KeyError):
        lines, passed = ["Component outcome evidence is missing or invalid.", next_step("evidence-invalid")], False
    message = "\n".join(lines)
    print(message)
    if os.environ.get("GITHUB_STEP_SUMMARY"):
        with Path(os.environ["GITHUB_STEP_SUMMARY"]).open("a") as stream:
            stream.write(message + "\n")
    if passed is None and os.environ.get("GITHUB_OUTPUT"):
        with Path(os.environ["GITHUB_OUTPUT"]).open("a") as stream:
            stream.write("release_outcome=reservation_superseded\n")
    return int(passed is False)


if __name__ == "__main__":
    raise SystemExit(main())
