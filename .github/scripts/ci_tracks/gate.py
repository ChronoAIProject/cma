"""Required checks distinguish intentional path skips from missing evidence."""

import json
import os
from pathlib import Path
import re
import sys

from ci_events import full_ci_required
from version_label import deployables

EXPECTED = {
    "repo": {"repo", "version-label"},
    "am": {"rust-test-build", "am", "am-images", "runtime-images", "omarchy", "approval-contract", "two-am-features"},
    "eg": {"rust-test-build", "eg", "eg-images", "runtime-images", "omarchy"},
    "web": {"web", "web-images"},
}
RUNTIME = {"runtime-images", "omarchy"}


def evaluate(track, needs, *, full=False, split=False):
    expected = EXPECTED[track]
    if split and track == "eg":
        expected = expected - RUNTIME
    if set(needs) != expected | {"changes"}:
        raise ValueError("missing or unexpected gate dependency")
    changes = needs["changes"]
    outputs = changes.get("outputs", {})
    if changes.get("result") != "success" or outputs.get("repo") != "true":
        raise ValueError("selection did not succeed")
    booleans = {key: outputs.get(key) for key in ("repo", "am", "eg", "web")}
    if any(value not in {"true", "false"} for value in booleans.values()):
        raise ValueError("invalid track selection schema")
    tracks = json.loads(outputs.get("test_tracks", "null"))
    if not isinstance(tracks, dict) or any(type(value) is not bool for value in tracks.values()):
        raise ValueError("invalid test track schema")
    if tracks != {key: value == "true" for key, value in booleans.items()}:
        raise ValueError("inconsistent test track outputs")
    if full and not all(tracks.values()):
        raise ValueError("full CI event requires every product track")
    deployables(json.loads(outputs.get("deployables", "null")))
    for rust_track in ("am", "eg"):
        packages = json.loads(outputs.get(rust_track + "_packages", "null"))
        if not isinstance(packages, list) or any(not isinstance(name, str) for name in packages) or packages != sorted(set(packages)):
            raise ValueError("invalid package selection schema")
        if any(not isinstance(name, str) or not re.fullmatch(r"[A-Za-z_][A-Za-z0-9_-]*", name) for name in packages):
            raise ValueError("invalid package name")
        if bool(packages) != (outputs[rust_track] == "true"):
            raise ValueError("package selection disagrees with selected track")
    selected = outputs.get(track)
    if selected not in {"true", "false"} or (track == "repo" and selected != "true"):
        raise ValueError("invalid track selection")
    contract_required = False
    if track == "am":
        approval_contract = outputs.get("approval_contract")
        if approval_contract not in {"true", "false"} or (tracks["am"] and approval_contract != "true"):
            raise ValueError("invalid approval contract selection")
        contract_required = approval_contract == "true"
        if needs["approval-contract"].get("result") != ("success" if contract_required else "skipped"):
            raise ValueError("approval contract did not reach its expected state")
        if contract_required and needs["rust-test-build"].get("result") != "success":
            raise ValueError("approval contract requires the Rust test archive")
        fixture = outputs.get("two_am_features")
        if fixture not in {"true", "false"} or (fixture == "true" and not tracks["am"]):
            raise ValueError("invalid two-Agent-Manager fixture selection")
        if needs["two-am-features"].get("result") != ("success" if fixture == "true" else "skipped"):
            raise ValueError("two-Agent-Manager fixture did not reach its expected state")
        expected = expected - {"approval-contract", "two-am-features"}
    if split:
        # The Agent Manager workflow owns the runtime checks for either Rust
        # product. An EG-only change still needs both runtime images to pass.
        required = expected if selected == "true" else set()
        if contract_required:
            required = required | {"rust-test-build"}
        if track == "am" and tracks["eg"]:
            required = required | RUNTIME
        failed = {
            job: needs[job].get("result", "missing")
            for job in expected
            if needs[job].get("result") != ("success" if job in required else "skipped")
        }
        if failed:
            raise ValueError("product children did not reach their expected state: " +
                             json.dumps(failed, sort_keys=True))
        return "passed" if required else "skipped-by-path"
    if selected == "false":
        # The archive can succeed for the other Rust product; this track's
        # own jobs must actually be skipped, rather than failing then hidden.
        own_jobs = expected - {"rust-test-build", "runtime-images", "omarchy"}
        if any(needs[job].get("result") != "skipped" for job in own_jobs):
            raise ValueError("unselected track has unexpected job results")
        return "passed" if contract_required else "skipped-by-path"
    failed = {job: needs[job].get("result", "missing") for job in expected
              if needs[job].get("result") != "success"}
    if failed:
        raise ValueError("required children did not succeed: " + json.dumps(failed, sort_keys=True))
    return "passed"


def main():
    try:
        event = json.loads(Path(os.environ['GITHUB_EVENT_PATH']).read_text())
        full = full_ci_required(event, os.environ['GITHUB_EVENT_NAME'], os.environ['GITHUB_REPOSITORY'])
        if sys.argv[2:] not in ([], ["--split"]):
            raise ValueError("invalid gate options")
        result = evaluate(sys.argv[1], json.loads(os.environ["NEEDS"]), full=full,
                          split=sys.argv[2:] == ["--split"])
    except (IndexError, KeyError, OSError, TypeError, ValueError) as error:
        print(f"::error::Track gate failed: {error}")
        return 1
    print(result)
    return 0


if __name__ == "__main__":
    sys.exit(main())
