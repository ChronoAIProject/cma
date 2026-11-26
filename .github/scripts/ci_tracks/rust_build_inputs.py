"""Reviewed inputs for exact Rust build programs and generated includes."""

import hashlib
from dataclasses import dataclass


@dataclass(frozen=True)
class BuildReview:
    target: str
    code: dict[str, str]
    reads: tuple[str, ...]
    generated_consumers: frozenset[str]


# The adapter generator reads one package-local declaration, passes its bytes
# to the pinned helper, and writes one Rust file in OUT_DIR. Pin the consumer
# as well so another computed include never inherits this review.
REVIEWED = {
    'api/crates/layer3-adapter': BuildReview(
        target='build.rs',
        code={
            'build.rs': 'ede7636145d454a9ce3a9f80c4a719a875b7731610b4562a9556d6b229631ee6',
            'build/runtime_versions.rs': '2cd68bcf8f9b4fcd1d49edd40cf1fa2d8b5e5641367995d44eb72ec07035d0b2',
            'src/runtime_versions.rs': '7e2573a99f6e2a8ecb540b80e964ddca72592b531a40f84f67d79562eadd68d5',
        },
        reads=('runtime-versions.json',),
        generated_consumers=frozenset({'src/runtime_versions.rs'}),
    ),
}


def inputs(root, directory, targets):
    review = REVIEWED.get(directory)
    if review is None or targets != {f'{directory}/{review.target}'}:
        return None
    base = root.resolve() / directory
    for relative, expected in review.code.items():
        path = base / relative
        if (not path.is_file() or path.resolve() != path
                or hashlib.sha256(path.read_bytes()).hexdigest() != expected):
            return None
    for relative in review.reads:
        path = base / relative
        if not path.is_file() or path.resolve() != path:
            return None
    return ({f'{directory}/{relative}' for relative in (*review.code, *review.reads)},
            {f'{directory}/{relative}' for relative in review.generated_consumers})
