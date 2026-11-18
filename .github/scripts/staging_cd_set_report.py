"""A skipped or superseded deployment job is never deployment acceptance."""
import os


def main():
    admission = os.environ.get("CD_ADMISSION_JOB_RESULT")
    deploy = os.environ.get("CD_DEPLOY_JOB_RESULT")
    if deploy == "success":
        message, code = "PASS: deployment job completed; consult its verified or disabled outcome", 0
    elif admission == "success" and deploy == "skipped" and os.environ.get("CD_SET_EXPECTED") == "false":
        message, code = "SKIP: no deployment set admitted", 0
    elif admission == "skipped" and deploy == "skipped":
        message, code = "SKIP: no coordinated deployment attempted", 0
    else:
        message, code = "FAIL: deployment not accepted; reconcile failed or superseded admission", 1
    print(message)
    if code:
        print("::error::Deployment did not complete. Maintainer reconciliation is required.")
    if os.environ.get("GITHUB_STEP_SUMMARY"):
        with open(os.environ["GITHUB_STEP_SUMMARY"], "a", encoding="utf-8") as stream:
            stream.write(message + "\n")
    return code


if __name__ == "__main__":
    raise SystemExit(main())
