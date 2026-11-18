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
    # Retain public keys and trust so later package verification still works.
    # Only the build-local private signing material and entropy are disposable.
    keyring = root / "etc/pacman.d/gnupg"
    private_keys = keyring / "private-keys-v1.d"
    if private_keys.exists():
        shutil.rmtree(private_keys)
    for path in keyring.glob("secring*"):
        if path.name == "secring" or path.name.startswith("secring."):
            path.unlink()
    (keyring / "random_seed").unlink(missing_ok=True)
    for name in ("var/log/pacman.log", "var/cache/ldconfig/aux-cache"):
        (root / name).unlink(missing_ok=True)


if __name__ == "__main__":
    normalize(sys.argv[1] if len(sys.argv) > 1 else "/")
