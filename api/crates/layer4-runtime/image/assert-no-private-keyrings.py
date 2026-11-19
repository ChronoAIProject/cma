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
    canonical = root / "etc/machine-id"
    runtime_state = "dev/shm/cma-machine-id"
    # Check the image path without following a symlink into the build host.
    for parent in ("dev", "dev/shm"):
        if (root / parent).is_symlink():
            return [parent]
    if os.path.lexists(root / runtime_state):
        found.append(runtime_state)
    for name in ("etc/machine-id", "var/lib/dbus/machine-id"):
        path = root / name
        try:
            metadata = path.lstat()
        except FileNotFoundError:
            continue
        if name == "var/lib/dbus/machine-id" and stat.S_ISLNK(metadata.st_mode):
            if path.readlink().as_posix() not in {"/etc/machine-id", "../../../etc/machine-id"}:
                found.append(name)
                continue
            path = canonical
            metadata = canonical.lstat()
        if stat.S_ISLNK(metadata.st_mode) and path == canonical:
            if path.readlink().as_posix() == "/" + runtime_state + "/machine-id":
                continue
        if not stat.S_ISREG(metadata.st_mode) or metadata.st_size != 0:
            found.append(name)
    return found


def populated_pacman_keyring(root):
    path = Path(root)
    for part in ("etc", "pacman.d", "gnupg"):
        path /= part
        if path.is_symlink() or (path.exists() and not path.is_dir()):
            return True
    return path.exists() and any(path.iterdir())


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
        initialized = populated_pacman_keyring(root)
    except (OSError, ValueError):
        print("Runtime image hygiene check failed: incomplete_inventory")
        return 2
    if count or identities:
        print(f"Runtime image hygiene check failed: private_entries={count}, machine_id_entries={identities}")
        return 1
    if initialized:
        print("Runtime image hygiene check failed: initialized_pacman_keyring")
        return 1
    if caches:
        print(f"Runtime image hygiene check failed: package_cache_entries={caches}")
        return 1
    print("Runtime image hygiene check passed: private_entries=0, machine_id_entries=0")
    return 0


if __name__ == "__main__":
    raise SystemExit(main(sys.argv[1] if len(sys.argv) > 1 else "/"))
