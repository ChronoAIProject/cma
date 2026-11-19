"""Redact the pinned device-response example and check the complete skill payload."""
import hashlib
from pathlib import Path
import re
import sys


DEVICES_SHA256 = "17c8045ec28413eeb8f116750658cf3f2277e5e0af665be38f02955634d07adf"
REFRESH_FIELD = re.compile(rb'("refresh_token"\s*:\s*")[0-9a-fA-F]{64}(")')
TOKEN_FIELD = re.compile(rb"""(?ix) (?<![a-z0-9_-]) [a-z0-9_-]*token ["']? \s* [:=] \s* ["']?
                               [0-9a-f]{64} (?![a-z0-9_]) """)


def prepare(skills):
    root = Path(skills).resolve(strict=True)
    devices = root / "nyxid/references/devices.md"
    if devices.resolve(strict=True) != devices:
        raise ValueError("Unexpected device documentation target")
    original = devices.read_bytes()
    if hashlib.sha256(original).hexdigest() != DEVICES_SHA256:
        raise ValueError("Review changed device documentation before packaging")
    updated, count = REFRESH_FIELD.subn(rb'\g<1><refresh_token>\g<2>', original)
    if count != 1:
        raise ValueError("Expected one refresh token example")
    devices.write_bytes(updated)
    # Inspect all regular skill inputs, including extensionless documentation.
    # No file name, field value, or neighboring text enters a build log.
    for path in root.rglob("*"):
        if not path.resolve(strict=True).is_relative_to(root):
            raise ValueError("Skill entry points outside its payload")
        if path.is_file() and TOKEN_FIELD.search(path.read_bytes()):
            raise ValueError("A packaged skill contains a literal token field")


if __name__ == "__main__":
    prepare(sys.argv[1])
    print("Packaged skill token examples verified")
