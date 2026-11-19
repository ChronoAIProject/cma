"""Discard build-only key material and normalize package installation metadata."""

from pathlib import Path
import re
import shutil
import sys


def normalize(root):
    root = Path(root)
    for path in (root / "var/lib/pacman/local").glob("*/desc"):
        value = path.read_text()
        updated, count = re.subn(r"(?m)^%INSTALLDATE%\n[0-9]+$", "%INSTALLDATE%\n0", value)
        if count != 1:
            raise ValueError("missing or ambiguous package installation date")
        path.write_text(updated)
    # The distro keyring packages under /usr/share remain the trust source.
    # Local signatures and ownertrust belong to the sandbox that creates them.
    keyring = root / "etc/pacman.d/gnupg"
    if keyring.exists():
        shutil.rmtree(keyring)
    keyring.mkdir(mode=0o755, parents=True)
    for name in ("var/log/pacman.log", "var/cache/ldconfig/aux-cache"):
        (root / name).unlink(missing_ok=True)
    # Repository indexes are build-time snapshots; future installs refresh them
    # with pacman -Syu. Preserve the separate installed-package database.
    sync = root / "var/lib/pacman/sync"
    if sync.exists():
        shutil.rmtree(sync)
    sync.mkdir(parents=True, exist_ok=True)


if __name__ == "__main__":
    normalize(sys.argv[1] if len(sys.argv) > 1 else "/")
