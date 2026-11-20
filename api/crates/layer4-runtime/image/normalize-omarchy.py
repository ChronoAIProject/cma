"""Prepare the installed Omarchy desktop for its runtime image layer."""

import hashlib
import re
import shutil
import subprocess
import sys
from pathlib import Path

# Documentation lines whose placeholder credentials match the image scanner's
# credential-URL rule: (path, reviewed file SHA-256, line SHA-256s, lines removed).
# A different file that still carries one of these lines needs a new review.
DOCUMENTATION_EXAMPLES = (
    (
        "usr/include/pcap/pcap.h",
        "cce0495dc7d3b8d070353868af32083275e78375247a10ef3a90d105f345532e",
        ("6045a2509d93e77dc96252b1ee2aedf99c0b4fc5648382624135b1a1f260fc53",),
        1,
    ),
    (
        "usr/share/gir-1.0/Gst-1.0.gir",
        "f8320303e6fc5a084486227af3b27c9dde4148e05867a97c7ba38d6e5c6c0335",
        ("5ccb4cb900cc5061f95d71042ad8a441bd02231e9416ddfd14a77e3919deeffb",),
        2,
    ),
    (
        "usr/share/perl5/core_perl/HTTP/Tiny.pm",
        "55f5f228d4bb293fc82b38f4933ea3fb60eb30adbee0554b9a2cb29c13765473",
        (
            "94faa8e5ffba2581a893e35154a8a9c0336ffa569696eadf89731e374a5f17d9",
            "3093c4322d516cdee479ad5812aa6885a36ede9d6f26c41d9e29db0ef8444e3d",
            "a84e4f4eeae2ec5c6d59aa9a304f66ac0e89d167e9323ca4802db44f7dd2a03c",
        ),
        3,
    ),
)
# Python's bundled ensurepip wheel is never rewritten: it stays the signed
# package member its pacman mtree records, and the image gate reviews its members.
ENSUREPIP_WHEEL = "usr/lib/python3.14/ensurepip/_bundled/pip-26.2.1-py3-none-any.whl"
OBJECT_SIGNING_BUNDLE = "etc/ca-certificates/extracted/objsign-ca-bundle.pem"

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


def remove_optional_gnutls_cli(root):
    # The standalone client embeds benchmark keys. Packages that link GnuTLS
    # still receive the distributor's unmodified libraries and package records.
    root = root.resolve()
    tool = root / "usr/bin/gnutls-cli"
    if tool.resolve() != tool:
        raise ValueError("Unexpected GnuTLS command target")
    if not tool.exists():
        return
    if not tool.is_file():
        raise ValueError("GnuTLS command is not a regular file")
    files = subprocess.run(["pacman", "--root", str(root), "-Ql", "gnutls"],
                           capture_output=True, text=True, check=True, timeout=20)
    if f"gnutls {tool}" not in files.stdout.splitlines():
        raise ValueError("The GnuTLS package does not own the optional command")
    tool.unlink()


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


def _package_file(root, relative, kind):
    path = root.resolve() / relative
    if path.resolve() != path:
        raise ValueError(f"Unexpected {kind} target: {relative}")
    if path.exists() and not path.is_file():
        raise ValueError(f"{kind.capitalize()} is not a regular file: {relative}")
    return path


def _without_lines(data, digests):
    lines = data.splitlines(keepends=True)
    retained = [line for line in lines if hashlib.sha256(line).hexdigest() not in digests]
    return b"".join(retained), len(lines) - len(retained)


def remove_documentation_examples(root):
    for relative, reviewed, digests, expected in DOCUMENTATION_EXAMPLES:
        path = _package_file(root, relative, "documentation example")
        if not path.exists():
            continue
        original = path.read_bytes()
        updated, removed = _without_lines(original, frozenset(digests))
        if removed == 0:
            continue
        # An updated source carrying these lines could move them into code.
        if removed != expected or hashlib.sha256(original).hexdigest() != reviewed:
            raise ValueError(f"Documentation example source requires review: {relative}")
        path.write_bytes(updated)


def remove_empty_object_signing_bundle(root):
    # Mozilla's store grants no object-signing trust, so p11-kit extracts an
    # empty bundle. update-ca-trust recreates it whenever trust changes.
    path = _package_file(root, OBJECT_SIGNING_BUNDLE, "object-signing trust bundle")
    if not path.exists():
        return
    if path.stat().st_size:
        raise ValueError("Object-signing trust bundle is not empty and requires review")
    path.unlink()


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
    remove_optional_gnutls_cli(root)
    remove_packaging_files(root)
    remove_portal_descriptions(root)
    remove_documentation_examples(root)
    remove_empty_object_signing_bundle(root)


if __name__ == "__main__":
    normalize(sys.argv[1] if len(sys.argv) > 1 else "/")
