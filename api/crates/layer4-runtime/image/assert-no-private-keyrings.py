"""Refuse private GnuPG storage, machine identities and package download caches."""

import os
from pathlib import Path
import stat
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


def machine_identities(root):
    root = Path(root)
    found = []
    for name in ("etc/machine-id", "var/lib/dbus/machine-id"):
        path = root / name
        try:
            metadata = path.lstat()
        except FileNotFoundError:
            continue
        if path.is_symlink():
            # Resolve only the standard D-Bus link inside the inspected image.
            if name != "var/lib/dbus/machine-id" or path.readlink().as_posix() not in {
                    "/etc/machine-id", "../../../etc/machine-id"}:
                found.append(name)
                continue
            metadata = (root / "etc/machine-id").lstat()
        if not stat.S_ISREG(metadata.st_mode) or metadata.st_size != 0:
            found.append(name)
    return found


def package_cache_entries(root):
    count = 0
    for name in ("var/lib/pacman/sync", "var/cache/pacman/pkg"):
        path = Path(root) / name
        if path.is_symlink() or (path.exists() and not path.is_dir()):
            raise ValueError("Invalid package cache directory")
        if path.is_dir():
            count += sum(1 for _ in path.iterdir())
    return count


def main(root="/"):
    try:
        count = len(private_keyrings(root))
        identities = len(machine_identities(root))
        caches = package_cache_entries(root)
    except (OSError, ValueError):
        print("Runtime image hygiene check failed: incomplete_inventory")
        return 2
    if count or identities:
        print(f"Runtime image hygiene check failed: private_entries={count}, machine_id_entries={identities}")
        return 1
    if caches:
        print(f"Runtime image hygiene check failed: package_cache_entries={caches}")
        return 1
    print("Runtime image hygiene check passed: private_entries=0, machine_id_entries=0")
    return 0


if __name__ == "__main__":
    raise SystemExit(main(sys.argv[1] if len(sys.argv) > 1 else "/"))
