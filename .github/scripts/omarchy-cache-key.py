"""Name the native dependency cache by its recipe, pinned arguments, mounted images and copied inputs."""

import hashlib
import json
import re
import shlex
import sys
from pathlib import Path

DOCKERFILE = "api/crates/layer4-runtime/Dockerfile"
TARGET = "omarchy-dependencies"
TRACKED = ("omarchy-bootstrap", "omarchy-base", TARGET)
DIGEST_REFERENCE = re.compile(r"[a-z0-9]+(?:[._-][a-z0-9]+)*(?::[0-9]+)?(?:/[a-z0-9]+(?:[._-][a-z0-9]+)*)+@sha256:[0-9a-f]{64}")
PLACEHOLDER = "@sha256:" + "0" * 64


def stage_lines(lines, name):
    starts = [i for i, line in enumerate(lines) if line.startswith("FROM ") and line.endswith(" AS " + name)]
    if len(starts) != 1:
        raise ValueError("expected one Omarchy stage: " + name)
    start = starts[0]
    stop = next((i for i in range(start + 1, len(lines)) if lines[i].startswith("FROM ")), len(lines))
    return lines[start:stop]


def global_arguments(lines):
    # Build arguments declared before the first stage, with their defaults.
    values = {}
    for line in lines:
        if line.startswith("FROM "):
            break
        if line.startswith("ARG ") and "=" in line:
            name, value = line[4:].split("=", 1)
            values[name] = value
    return values


def track_inputs(root, stage, inputs, arguments, defaults):
    for line in stage[1:]:
        if not line or line.startswith("#"):
            continue
        if line.startswith("ARG "):
            # A stage may consume only an argument pinned by a global default.
            name = line[4:]
            if name not in defaults:
                raise ValueError("dependency-stage argument must have a pinned global default")
            arguments[name] = defaults[name]
        elif line.startswith("COPY "):
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


def mounted_images(lines, stages, defaults):
    # A stage that a tracked RUN mounts must be one image pinned by digest; its
    # resolved reference names the cache, like a copied input does.
    images = {}
    for line in stages:
        if not line.startswith("RUN "):
            continue
        for option in re.findall(r"--mount=(\S+)", line):
            fields = dict(field.split("=", 1) for field in option.split(",") if "=" in field)
            name = fields.get("from")
            if name is None or name in TRACKED:
                continue
            stage = [entry for entry in stage_lines(lines, name) if entry and not entry.startswith("#")]
            match = re.fullmatch(r"FROM (\S+) AS " + re.escape(name), stage[0]) if len(stage) == 1 else None
            if match is None:
                raise ValueError("mounted stage must be exactly one pinned image: " + name)
            reference = match[1]
            variable = re.fullmatch(r"\$\{([A-Za-z_][A-Za-z0-9_]*)\}", reference)
            if variable:
                if variable[1] not in defaults:
                    raise ValueError("mounted image argument must have a pinned global default")
                reference = defaults[variable[1]]
            if not DIGEST_REFERENCE.fullmatch(reference):
                raise ValueError("mounted image must be pinned by digest: " + name)
            if reference.endswith(PLACEHOLDER):
                raise ValueError("replace the placeholder digest of the mounted image: " + name)
            images[name] = reference
    return images


def manifest(root):
    root = Path(root).resolve()
    lines = (root / DOCKERFILE).read_text().splitlines()
    dependencies = stage_lines(lines, TARGET)
    defaults = global_arguments(lines)
    inputs, arguments = {}, {}
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
        track_inputs(root, bootstrap, inputs, arguments, defaults)
        stages = bootstrap + base + dependencies
    track_inputs(root, dependencies, inputs, arguments, defaults)
    if not inputs:
        raise ValueError("dependency stage has no source inputs")
    return {"schema": 1, "platform": "linux/amd64", "target": TARGET,
            "recipe": "\n".join(stages), "arguments": arguments, "inputs": inputs,
            "images": mounted_images(lines, stages, defaults)}


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
