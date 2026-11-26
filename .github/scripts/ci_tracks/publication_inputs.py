"""Publication inputs that select images from outside their build contexts."""

import re
import subprocess

VERSION_FILE = 'api/crates/layer3-adapter/runtime-versions.json'
RECIPE = 'api/crates/layer4-runtime/Dockerfile'
CONTRACT_FILES = frozenset({VERSION_FILE, RECIPE})
CONTRACT_IMAGES = frozenset({'cma-agent-manager', 'cma-runtime', 'cma-runtime-omarchy'})
DEBIAN_SNAPSHOT = re.compile(r'^ARG DEBIAN_SNAPSHOT=[0-9]{8}T[0-9]{6}Z$', re.MULTILINE)


def packaging_signature(recipe):
    from docker_inputs import instructions

    # All recipe text except the one global package-index default must match.
    # Comparing labels across all stages cannot establish either target's labels.
    declarations = list(DEBIAN_SNAPSHOT.finditer(recipe))
    if len(declarations) != 1:
        return None
    declaration = declarations[0]
    prefix = recipe[:declaration.start()]
    if any(op != 'ARG' for op, _ in instructions(prefix)):
        return None
    return prefix + 'ARG DEBIAN_SNAPSHOT' + recipe[declaration.end():]


def contract_images(paths, read_recipes=None):
    # Publisher, scanner and gate code is not an image input. A lane whose
    # latest final contains every input change stays current; staging
    # allocation retries a lane whose latest attempt did not finalize.
    touched = CONTRACT_FILES & set(paths)
    if not touched:
        return frozenset()
    if VERSION_FILE not in touched and read_recipes is not None:
        try:
            previous, current = read_recipes()
            previous_signature = packaging_signature(previous)
            if previous_signature is not None and previous_signature == packaging_signature(current):
                return frozenset()
        except (OSError, ValueError, subprocess.SubprocessError):
            # An unavailable or ambiguous declaration cannot narrow the group.
            pass
    return CONTRACT_IMAGES
