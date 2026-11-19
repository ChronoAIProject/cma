"""A skipped or superseded deployment job is never deployment acceptance."""
import os

from staging_cd_component_admission import validate_failures
from staging_cd_receipts import canonical
from staging_cd_set import document, validate_set


def main():
    admission = os.environ.get("CD_ADMISSION_JOB_RESULT")
    deploy = os.environ.get("CD_DEPLOY_JOB_RESULT")
    publication = os.environ.get("CD_PUBLICATION_JOB_RESULT")
    try:
        failures = validate_failures(document(os.environ.get("CD_COMPONENT_FAILURES") or "{}"))
        value = os.environ.get("CD_RELEASE_SET")
        admitted = sorted(validate_set(document(value))["receipts"]) if value else []
        allowed = {None, "", "success", "failure", "cancelled", "skipped"}
        if any(state not in allowed for state in (admission, deploy, publication)):
            raise ValueError("Invalid job conclusion")
    except (ValueError, TypeError, KeyError):
        print("::error::Component outcome evidence is invalid; no deployment acceptance is claimed.")
        return 1
    print("CD_RESULT " + canonical({"schema": "cma-component-run-result-v1",
          "publication_result": publication or "unknown", "admission_result": admission or "unknown",
          "deployment_result": deploy or "unknown", "admitted": admitted, "failed": failures}))
    if deploy == "success":
        message, code = "PASS: deployment job completed; consult its verified or disabled outcome", 0
    elif admission == "success" and deploy == "skipped" and os.environ.get("CD_SET_EXPECTED") == "false":
        message, code = "SKIP: no deployment set admitted", 0
    elif admission == "skipped" and deploy == "skipped":
        message, code = "SKIP: no coordinated deployment attempted", 0
    else:
        message, code = "FAIL: deployment not accepted; reconcile failed or superseded admission", 1
    lines = [f"FAIL component={component} reason={reason}" for component, reason in sorted(failures.items())]
    if failures or publication in ("failure", "cancelled"):
        message, code = "FAIL: publication or qualification failed for one or more components; consult admitted deployment outcome", 1
    for line in lines:
        print(line)
    print(message)
    if code:
        print("::error::One or more release outcomes failed. Maintainer reconciliation is required.")
    if os.environ.get("GITHUB_STEP_SUMMARY"):
        with open(os.environ["GITHUB_STEP_SUMMARY"], "a", encoding="utf-8") as stream:
            stream.write("\n".join([*lines, message]) + "\n")
    return code


if __name__ == "__main__":
    raise SystemExit(main())
