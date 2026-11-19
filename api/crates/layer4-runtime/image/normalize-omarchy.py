"""Prepare the installed Omarchy desktop for its runtime image layer."""

import hashlib
import re
import shutil
import sys
from pathlib import Path

PORTAL_DESCRIPTIONS = (
    (
        "org.freedesktop.impl.portal.Secret.xml",
        "6a73aa125c66e286473cea2edc4d0281360ad08efc4eb2872200bdb3b1729d04",
    ),
    (
        "org.freedesktop.portal.Secret.xml",
        "790fb228b8121353b2a5cb962cce54b1c5af6928c5d250b28b14e6ba2bd511c9",
    ),
)
XML_COMMENT = re.compile(rb"<!--.*?-->", re.DOTALL)


def remove_portal_descriptions(root):
    # The two pinned comments describe the interface. Preserve the license,
    # method documentation and every byte of the D-Bus API declarations.
    directory = root.resolve() / "usr/share/dbus-1/interfaces"
    for name, expected in PORTAL_DESCRIPTIONS:
        path = directory / name
        if path.resolve() != path:
            raise ValueError("Unexpected D-Bus portal documentation target")
        if not path.exists():
            continue
        original = path.read_bytes()
        matches = [match for match in XML_COMMENT.finditer(original)
                   if hashlib.sha256(match[0]).hexdigest() == expected]
        if len(matches) != 1:
            raise ValueError("Review changed D-Bus portal description before packaging")
        match, = matches
        path.write_bytes(original[:match.start()] + original[match.end():])


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
    remove_portal_descriptions(root)


if __name__ == "__main__":
    normalize(sys.argv[1] if len(sys.argv) > 1 else "/")
