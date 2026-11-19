"""Name the native dependency cache by its recipe and every copied input."""

import hashlib
import json
import shlex
import sys
from pathlib import Path

DOCKERFILE = "api/crates/layer4-runtime/Dockerfile"
TARGET = "omarchy-dependencies"


def stage_lines(lines, name):
    starts = [i for i, line in enumerate(lines) if line.startswith("FROM ") and line.endswith(" AS " + name)]
    if len(starts) != 1:
        raise ValueError("expected one Omarchy stage: " + name)
    start = starts[0]
    stop = next((i for i in range(start + 1, len(lines)) if lines[i].startswith("FROM ")), len(lines))
    return lines[start:stop]


def track_inputs(root, stage, inputs):
    for line in stage[1:]:
        if not line or line.startswith("#"):
            continue
        if line.startswith("COPY "):
            words = shlex.split(line)
            if len(words) < 3 or any(word.startswith("--") for word in words[1:]):
                raise ValueError("dependency COPY must name explicit local files")
            for name in words[1:-1]:
                path = root / name
                if path.is_symlink() or not path.is_file() or not path.resolve().is_relative_to(root):
                    raise ValueError("dependency input must be a regular repository file")
                inputs[name] = hashlib.sha256(path.read_bytes()).hexdigest()
        elif not line.startswith("RUN "):
            raise ValueError("unsupported dependency-stage instruction; review cache inputs")


def manifest(root):
    root = Path(root).resolve()
    lines = (root / DOCKERFILE).read_text().splitlines()
    dependencies = stage_lines(lines, TARGET)
    inputs = {}
    if dependencies[0].startswith("FROM archlinux:base@sha256:"):
        # Historical builds used the pinned image directly.
        stages = dependencies
    else:
        if dependencies[0] != "FROM omarchy-base AS " + TARGET:
            raise ValueError("dependency stage must use the assembled Omarchy base")
        bootstrap = stage_lines(lines, "omarchy-bootstrap")
        if not bootstrap[0].startswith("FROM archlinux:base@sha256:"):
            raise ValueError("bootstrap stage must retain its pinned Arch input")
        base = stage_lines(lines, "omarchy-base")
        if [line for line in base if line and not line.startswith("#")] != [
            "FROM scratch AS omarchy-base", "COPY --link --from=omarchy-bootstrap /payload/ /",
        ]:
            raise ValueError("Omarchy base must copy only the assembled package root")
        track_inputs(root, bootstrap, inputs)
        stages = bootstrap + base + dependencies
    track_inputs(root, dependencies, inputs)
    if not inputs:
        raise ValueError("dependency stage has no source inputs")
    return {"schema": 1, "platform": "linux/amd64", "target": TARGET,
            "recipe": "\n".join(stages), "inputs": inputs}


def scope(value):
    encoded = json.dumps(value, sort_keys=True, separators=(",", ":")).encode()
    return "runtime-omarchy-deps-" + hashlib.sha256(encoded).hexdigest()


def main():
    root = Path(__file__).resolve().parents[2]
    value = manifest(root)
    value["scope"] = scope(value)
    evidence = Path(sys.argv[1])
    evidence.parent.mkdir(parents=True, exist_ok=True)
    evidence.write_text(json.dumps(value, indent=2, sort_keys=True) + "\n")
    print("scope=" + value["scope"])


if __name__ == "__main__":
    main()
