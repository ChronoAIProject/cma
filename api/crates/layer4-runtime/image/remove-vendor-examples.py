"""Omit reviewed source examples without dropping their executable modules."""

import hashlib
from pathlib import Path
import sys


EXAMPLES = {
    "perl": (
        "usr/share/perl/5.40.1/HTTP/Tiny.pm",
        "b0e085ec36f0b5d19d0f08bd2e88df793ffef7cbfa397c524f875ca68c4c47a0",
        (
            "94faa8e5ffba2581a893e35154a8a9c0336ffa569696eadf89731e374a5f17d9",
            "3093c4322d516cdee479ad5812aa6885a36ede9d6f26c41d9e29db0ef8444e3d",
            "a84e4f4eeae2ec5c6d59aa9a304f66ac0e89d167e9323ca4802db44f7dd2a03c",
        ),
    ),
    "npm": (
        "usr/local/lib/node_modules/npm/node_modules/hosted-git-info/lib/parse-url.js",
        "3975a74ee1830772ada32f51599c293603524c7a077e36375957aafd1df6955a",
        (
            "f4366151028e0da3a24cafc70feb8d636849085f84a54327472d0792a5b6e998",
            "4722b1564ea74e2b024296957d733f2164037c7f14753d56ec6616b1479bfa9a",
        ),
    ),
}


def remove_examples(root, package):
    relative, reviewed_source, examples = EXAMPLES[package]
    path = Path(root).resolve() / relative
    if path.resolve() != path:
        raise ValueError(f"Unexpected vendor example file target: {relative}")
    if not path.exists():
        return

    original = path.read_bytes()
    lines = original.splitlines(keepends=True)
    retained = [line for line in lines if hashlib.sha256(line).hexdigest() not in examples]
    removed = len(lines) - len(retained)
    if removed == 0:
        return
    # An updated source file carrying these examples could move them into code.
    if removed != len(examples) or hashlib.sha256(original).hexdigest() != reviewed_source:
        raise ValueError(f"Vendor example source requires review: {relative}")

    path.write_bytes(b"".join(retained))


if __name__ == "__main__":
    remove_examples("/", sys.argv[1])
