"""Refuse runtime filesystems that retain GnuPG private storage."""

import os
from pathlib import Path
import sys


def secret_location(path):
    return ("private-keys-v1.d" in path.parts or path.name == "secring"
            or path.name.startswith("secring."))


def private_keyrings(root):
    root = Path(root)
    if root.is_symlink() or not root.is_dir():
        raise ValueError("Invalid filesystem root")
    found = set()

    def unreadable(error):
        raise error

    for directory, directories, files in os.walk(root, followlinks=False, onerror=unreadable):
        parent = Path(directory)
        # These are the build process's virtual mounts, not image payloads.
        if parent == root:
            directories[:] = [name for name in directories if name not in {"dev", "proc", "sys"}]
        for name in directories:
            path = parent / name
            if path.is_symlink() and secret_location(path.relative_to(root)):
                found.add(path.relative_to(root).as_posix())
        for name in files:
            path = (parent / name).relative_to(root)
            if secret_location(path):
                found.add(path.as_posix())
    return sorted(found)


def main(root="/"):
    try:
        count = len(private_keyrings(root))
    except (OSError, ValueError):
        print("Runtime private-keyring check failed: incomplete_inventory")
        return 2
    if count:
        print(f"Runtime private-keyring check failed: private_entries={count}")
        return 1
    print("Runtime private-keyring check passed: private_entries=0")
    return 0


if __name__ == "__main__":
    raise SystemExit(main(sys.argv[1] if len(sys.argv) > 1 else "/"))
