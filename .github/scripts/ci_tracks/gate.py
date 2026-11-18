"""Required checks distinguish intentional path skips from missing evidence."""

import json
import os
import re
import sys

from version_label import deployables

EXPECTED = {
    "repo": {"structure", "secrets", "version-label"},
    "am": {"lint", "test-build", "tests", "timing", "mongo", "docker", "scan", "runtime", "omarchy"},
    "eg": {"lint", "test-build", "tests", "timing", "mongo", "docker", "scan"},
    "web": {"install", "format", "lint", "typecheck", "tests", "build", "styles", "browser", "audit", "docker", "scan"},
}


def evaluate(track, needs):
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
    required = {job: selected == "true" for job in expected}
    if track == "am":
        # Shared runtime coverage executes once, in the AM workflow. Required
        # branch protection includes this gate even for an EG-only change.
        for job in ("runtime", "omarchy"):
            required[job] = outputs["am"] == "true" or outputs["eg"] == "true"
    failed = {job: needs[job].get("result", "missing") for job in expected
              if needs[job].get("result") != ("success" if required[job] else "skipped")}
    if failed:
        raise ValueError("required children did not succeed: " + json.dumps(failed, sort_keys=True))
    return "passed" if any(required.values()) else "skipped-by-path"


def main():
    try:
        result = evaluate(sys.argv[1], json.loads(os.environ["NEEDS"]))
    except (IndexError, KeyError, TypeError, ValueError) as error:
        print(f"::error::Track gate failed: {error}")
        return 1
    print(result)
    return 0


if __name__ == "__main__":
    sys.exit(main())
