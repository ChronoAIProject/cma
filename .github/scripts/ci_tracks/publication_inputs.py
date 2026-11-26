"""Publication inputs that select images from outside their build contexts."""

import shlex
import subprocess

VERSION_FILE = 'api/crates/layer3-adapter/runtime-versions.json'
RECIPE = 'api/crates/layer4-runtime/Dockerfile'
CONTRACT_FILES = frozenset({VERSION_FILE, RECIPE})
CONTRACT_IMAGES = frozenset({'cma-agent-manager', 'cma-runtime', 'cma-runtime-omarchy'})
LABELS = frozenset({'dev.chrono.cma.runtime-contract', 'dev.chrono.cma.runtime-bootstrap',
                    'dev.chrono.cma.home-layout'})


def runtime_labels(text, require):
    labels = {key: set() for key in LABELS}
    for line in text.replace('\\\n', ' ').splitlines():
        if not line.strip() or line.split(maxsplit=1)[0].upper() != 'LABEL':
            continue
        for word in shlex.split(line, comments=True)[1:]:
            key, separator, value = word.partition('=')
            require(separator, 'Runtime contract label syntax is unmodeled')
            if key in labels:
                require(value.isascii() and value.isdecimal(), 'Runtime contract label is unmodeled')
                labels[key].add(value)
    require(all(len(values) == 1 for values in labels.values()), 'Runtime contract labels are ambiguous')
    return tuple((key, next(iter(values))) for key, values in sorted(labels.items()))


def require_label(condition, message):
    if not condition:
        raise ValueError(message)


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
            if runtime_labels(previous, require_label) == runtime_labels(current, require_label):
                return frozenset()
        except (OSError, ValueError, subprocess.SubprocessError):
            # An unavailable or ambiguous declaration cannot narrow the group.
            pass
    return CONTRACT_IMAGES
