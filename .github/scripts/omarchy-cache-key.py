#!/usr/bin/env python3
"""Name the native dependency cache by its recipe and every copied input."""

import hashlib
import json
from pathlib import Path
import shlex
import sys

DOCKERFILE = "infra/runtime/Dockerfile"
TARGET = "omarchy-dependencies"


def manifest(root):
    root = Path(root).resolve()
    recipe = (root / DOCKERFILE).read_text()
    lines = recipe.splitlines()
    starts = [i for i, line in enumerate(lines) if line.endswith(" AS " + TARGET)]
    if len(starts) != 1:
        raise ValueError("expected one pinned Omarchy dependency stage")
    start = starts[0]
    stop = next((i for i in range(start + 1, len(lines)) if lines[i].startswith("FROM ")), len(lines))
    stage = lines[start:stop]
    if not stage[0].startswith("FROM archlinux:base@sha256:"):
        raise ValueError("dependency stage must retain its pinned Arch base")
    inputs = {}
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
    if not inputs:
        raise ValueError("dependency stage has no source inputs")
    return {"schema": 1, "platform": "linux/amd64", "target": TARGET,
            "recipe": "\n".join(stage), "inputs": inputs}


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
