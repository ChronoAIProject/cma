"""Require every named Web track job to succeed.

CI / Web sees only the reusable Web workflow's aggregate result, which cannot
tell a skipped, cancelled or removed job from a passing one. This job sees
each one by name.
"""

import json
import os
import sys

JOBS = ("web", "web-journeys", "web-lifecycle", "web-design", "web-conversation")


def check(needs):
    """Return why the Web track is incomplete, or None when every job succeeded."""
    if not isinstance(needs, dict) or set(needs) != set(JOBS):
        return "Web result must need exactly " + ", ".join(JOBS)
    failed = [name for name in JOBS
              if not isinstance(needs[name], dict) or needs[name].get("result") != "success"]
    return "Web jobs did not succeed: " + ", ".join(failed) if failed else None


def main():
    try:
        needs = json.loads(os.environ["NEEDS"])
    except (KeyError, ValueError):
        needs = None
    error = check(needs)
    if error:
        print(f"::error title=Web track incomplete::{error}")
        return 1
    print("PASS web result: all " + str(len(JOBS)) + " Web jobs succeeded")
    return 0


if __name__ == "__main__":
    sys.exit(main())
