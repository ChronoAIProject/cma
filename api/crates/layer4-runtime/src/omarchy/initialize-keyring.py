#!/usr/bin/python -I
"""Create the rootless sandbox's pacman trust database from signed distro inputs."""

import fcntl
import os
from pathlib import Path
import re
import shutil
import stat
import subprocess
import sys


# The sandbox has a private IPC namespace. This location survives supervisor
# restarts, works with a read-only root, and never enters a shared home snapshot.
STATE = Path("/dev/shm/cma-pacman")
DISTRIBUTION = Path("/usr/share/pacman/keyrings")
PACKAGES = ("archlinux", "omarchy")
IDENTITY = "Pacman Keyring Master Key <pacman@localhost>"


class KeyringError(ValueError):
    """Contains only fixed, value-free diagnostics."""


def records(path, pattern):
    for line in path.read_text(encoding="ascii").splitlines():
        if not line.strip() or line.startswith("#"):
            continue
        match = re.fullmatch(pattern, line)
        if not match:
            raise KeyringError("invalid distribution keyring record")
        yield match[1]


def invoke(operation, tool, *arguments, data=None):
    # GnuPG diagnostics can contain key identifiers. Only the operation's name
    # crosses the supervisor log boundary; all trust inputs are local.
    result = subprocess.run([tool, *arguments], input=data if data is not None else b"",
                            stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL,
                            env={"PATH": "/usr/bin", "HOME": "/home/cma", "LC_ALL": "C"},
                            check=False, timeout=60)
    if result.returncode:
        raise KeyringError(f"{operation} failed")


def verify(ring):
    # pacman-key expects the classic pubring file, even for read-only commands.
    if not all((ring / name).is_file() for name in ("pubring.gpg", "trustdb.gpg")):
        raise KeyringError("incomplete sandbox keyring")
    invoke("verify master key", "/usr/bin/gpg", "--homedir", str(ring), "--batch", "--list-secret-keys", IDENTITY)


def populate(ring, distribution):
    def gpg(operation, *arguments, data=None):
        invoke(operation, "/usr/bin/gpg", "--homedir", str(ring), "--batch", "--no-auto-key-retrieve",
               "--auto-key-locate", "clear", *arguments, data=data)

    (ring / "pubring.gpg").touch(mode=0o600)
    (ring / "gpg-agent.conf").write_text("disable-scdaemon\n")
    try:
        # Stock pacman-key requires root even with --gpgdir. Use the same trust
        # inputs as --populate without giving the sandbox additional privileges.
        gpg("generate master key", "--pinentry-mode", "loopback", "--passphrase", "", "--quick-generate-key",
            IDENTITY, "rsa4096", "sign", "0")
        for package in PACKAGES:
            gpg("import distribution keys", "--import", str(distribution / (package + ".gpg")))
        for package in PACKAGES:
            trust = distribution / (package + "-trusted")
            masters = list(records(trust, r"([A-Fa-f0-9]{40}):[1-6]:"))
            if not masters:
                raise KeyringError("distribution trust anchors missing")
            for fingerprint in masters:
                gpg("sign distribution trust anchor", "--quick-lsign-key", fingerprint)
            gpg("import distribution ownertrust", "--import-ownertrust", str(trust))
        for package in PACKAGES:
            for fingerprint in records(distribution / (package + "-revoked"), r"([A-Fa-f0-9]{40})"):
                gpg("disable revoked key", "--command-fd", "0", "--no-auto-check-trustdb", "--edit-key", fingerprint,
                    data=b"disable\nquit\n")
        gpg("check trust database", "--check-trustdb")
        verify(ring)
    finally:
        invoke("stop keyring agent", "/usr/bin/gpgconf", "--homedir", str(ring), "--kill", "all")


def initialize(state=STATE, distribution=DISTRIBUTION):
    if os.getuid() != 10001:
        raise KeyringError("unexpected sandbox user")
    state.mkdir(mode=0o700, exist_ok=True)
    fd = os.open(state, os.O_RDONLY | os.O_DIRECTORY | os.O_NOFOLLOW)
    try:
        info = os.fstat(fd)
        if info.st_uid != os.getuid() or stat.S_IMODE(info.st_mode) != 0o700:
            raise KeyringError("unsafe sandbox keyring directory")
        fcntl.flock(fd, fcntl.LOCK_EX)
        ring = state / "gnupg"
        if ring.is_symlink():
            raise KeyringError("invalid sandbox keyring")
        if ring.exists():
            verify(ring)
            return
        # A killed initialization must never leave a partially trusted keyring
        # visible to pacman. The fixed staging directory is private and locked.
        pending = state / "initializing"
        if pending.is_symlink():
            raise KeyringError("invalid staging directory")
        if pending.exists():
            invoke("stop interrupted keyring agent", "/usr/bin/gpgconf", "--homedir", str(pending), "--kill", "all")
            shutil.rmtree(pending)
        pending.mkdir(mode=0o700)
        try:
            populate(pending, distribution)
            pending.rename(ring)
        finally:
            if pending.exists():
                shutil.rmtree(pending)
    finally:
        os.close(fd)


if __name__ == "__main__":
    os.umask(0o077)
    try:
        initialize()
    except KeyringError as error:
        sys.exit(f"Could not initialize sandbox pacman keyring: {error}")
    except (OSError, ValueError, subprocess.SubprocessError):
        sys.exit("Could not initialize sandbox pacman keyring")
