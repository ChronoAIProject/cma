"""Remove build-local signing authority while retaining package verification."""
import argparse
import hashlib
import os
import shutil
from pathlib import Path


def fingerprint(path):
    if path.is_symlink() or not path.is_file() or not path.stat().st_size:
        raise ValueError("Invalid public keyring state")
    return hashlib.sha256(path.read_bytes()).digest(), path.stat().st_mode


def strip_private_keyring(root):
    root = root.resolve(strict=True)
    keyring = root / "etc/pacman.d/gnupg"
    for path in (root / "etc", root / "etc/pacman.d", keyring):
        if path.is_symlink() or not path.is_dir():
            raise ValueError("Invalid keyring directory")
    public = [keyring / name for name in ("pubring.gpg", "pubring.kbx")
              if (keyring / name).exists() or (keyring / name).is_symlink()]
    if not public:
        raise ValueError("Missing public keyring")
    public.append(keyring / "trustdb.gpg")
    before = {path: fingerprint(path) for path in public}
    removals = set()
    directories = []
    for name in ("private-keys-v1.d", "openpgp-revocs.d"):
        path = keyring / name
        if path.is_symlink():
            raise ValueError("Redirected private keyring directory")
        if path.exists():
            if not path.is_dir():
                raise ValueError("Invalid private keyring directory")
            directories.append(path)
            removals.update(path.iterdir())
    # Select all backup/socket names before mutation, without following links.
    visited = 0
    for directory, children, files in os.walk(keyring, followlinks=False):
        visited += 1
        if visited > 10000:
            raise ValueError("Keyring traversal exceeds its bound")
        for name in children + files:
            if name.endswith("~") or name.startswith("S."):
                removals.add(Path(directory) / name)
        children[:] = [name for name in children
                       if not (Path(directory) / name).is_symlink()]
    legacy = keyring / "secring.gpg"
    if legacy.exists() or legacy.is_symlink():
        removals.add(legacy)
    count = 0
    for path in sorted(removals, key=lambda item: len(item.parts), reverse=True):
        if path.is_symlink() or not path.is_dir():
            path.unlink(missing_ok=True)
        else:
            shutil.rmtree(path)
        count += 1
    if any(any(path.iterdir()) for path in directories):
        raise ValueError("Private keyring cleanup incomplete")
    if any(fingerprint(path) != expected for path, expected in before.items()):
        raise ValueError("Public keyring changed during cleanup")
    return count


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--root", required=True, type=Path)
    args = parser.parse_args()
    try:
        count = strip_private_keyring(args.root)
    except (ValueError, OSError):
        print("pacman-private-keyring-strip=FAIL")
        return 1
    print("pacman-private-keyring-strip=PASS removed=" + str(count))
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
