#!/usr/bin/python3 -I
"""Publish one machine ID in the sandbox's private IPC filesystem."""
import os
from pathlib import Path
import re
import stat
import sys
import tempfile
import uuid


STATE = Path("/dev/shm/cma-machine-id")


def initialize(directory):
    directory.mkdir(mode=0o700, exist_ok=True)
    fd = os.open(directory, os.O_RDONLY | os.O_DIRECTORY | os.O_NOFOLLOW)
    try:
        parent = os.fstat(fd)
        if parent.st_uid != os.geteuid() or stat.S_IMODE(parent.st_mode) not in {0o700, 0o711}:
            raise ValueError("invalid identity directory")
        # Other local users can read the standard ID without listing the directory.
        os.fchmod(fd, 0o711)
    finally:
        os.close(fd)
    target = directory / "machine-id"
    # The private IPC mount survives supervisor restarts and is never cloned
    # with a home. Link a complete inode so concurrent initializers agree.
    with tempfile.NamedTemporaryFile(dir=directory) as candidate:
        candidate.write((uuid.uuid4().hex + "\n").encode("ascii"))
        candidate.flush()
        os.fchmod(candidate.fileno(), 0o444)
        try:
            os.link(candidate.name, target)
        except FileExistsError:
            pass  # The winning inode must pass the same checks below.
    fd = os.open(target, os.O_RDONLY | os.O_NOFOLLOW | os.O_NONBLOCK)
    with os.fdopen(fd, "rb") as identity:
        info = os.fstat(fd)
        if not stat.S_ISREG(info.st_mode) or info.st_uid != os.geteuid() or stat.S_IMODE(info.st_mode) != 0o444:
            raise ValueError("invalid identity file")
        raw = identity.read(34)
        if not re.fullmatch(rb"[0-9a-f]{32}\n", raw) or raw == b"0" * 32 + b"\n":
            raise ValueError("invalid identity format")


if __name__ == "__main__":
    try:
        initialize(STATE)
    except (OSError, ValueError):
        sys.exit("Sandbox machine identity initialization failed")
    print("Sandbox machine identity ready")
