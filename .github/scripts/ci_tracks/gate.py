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
    "am": {"rust-test-build", "am", "am-images", "runtime-images", "omarchy"},
    "eg": {"rust-test-build", "eg", "eg-images", "runtime-images", "omarchy"},
    "web": {"web", "web-images"},
}


def evaluate(track, needs, *, full=False):
    expected = EXPECTED[track]
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
    if selected == "false":
        # The archive can succeed for the other Rust product; this track's
        # own jobs must actually be skipped, rather than failing then hidden.
        own_jobs = expected - {"rust-test-build", "runtime-images", "omarchy"}
        if any(needs[job].get("result") != "skipped" for job in own_jobs):
            raise ValueError("unselected track has unexpected job results")
        return "skipped-by-path"
    failed = {job: needs[job].get("result", "missing") for job in expected
              if needs[job].get("result") != "success"}
    if failed:
        raise ValueError("required children did not succeed: " + json.dumps(failed, sort_keys=True))
    return "passed"


def main():
    try:
        event = json.loads(Path(os.environ['GITHUB_EVENT_PATH']).read_text())
        full = full_ci_required(event, os.environ['GITHUB_EVENT_NAME'], os.environ['GITHUB_REPOSITORY'])
        result = evaluate(sys.argv[1], json.loads(os.environ["NEEDS"]), full=full)
    except (IndexError, KeyError, OSError, TypeError, ValueError) as error:
        print(f"::error::Track gate failed: {error}")
        return 1
    print(result)
    return 0


if __name__ == "__main__":
    sys.exit(main())
