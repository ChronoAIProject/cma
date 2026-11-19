"""Prepare the installed Omarchy desktop for its runtime image layer."""

from pathlib import Path
import re
import shutil
import sys


def remove_packaging_files(root):
    # Keep NumPy's runtime testing API, Omarchy's launchers, package licenses
    # and ordinary help pages. Only these upstream suites and manuals are omitted.
    patterns = [
        "usr/share/omarchy/test",
        "usr/lib/python*/site-packages/numpy/random/tests",
        "usr/share/doc/zmq/zmq_curve.html",
        "usr/share/man/man7/zmq_curve.7.gz",
        "usr/share/man/man1/git-credential-store.1.gz",
        "usr/share/man/man3/HTTP::Tiny.3perl.gz",
        "usr/share/man/man5/foot.ini.5.gz",
        "usr/share/git/credential/libsecret/Makefile",
        "usr/share/git/credential/osxkeychain/git-credential-osxkeychain.c",
        "usr/share/perl5/core_perl/pod/perlguts.pod",
        "var/cache/pacman/pkg/*",
        "root/.cache/pip",
    ]
    for manual in ("ffmpeg-all", "ffmpeg-protocols", "ffplay-all", "ffprobe-all"):
        patterns.extend((f"usr/share/doc/ffmpeg/{manual}.html", f"usr/share/man/man1/{manual}.1.gz"))
    for pattern in patterns:
        for path in root.glob(pattern):
            if path.is_symlink() or not path.is_dir():
                path.unlink()
            else:
                shutil.rmtree(path)


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
    remove_packaging_files(root)


if __name__ == "__main__":
    normalize(sys.argv[1] if len(sys.argv) > 1 else "/")
