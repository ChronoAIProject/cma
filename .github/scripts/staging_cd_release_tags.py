"""Resolve immutable tag objects from the release checkout after a live ref read.

Mutable refs are always read live from GitHub. This module only supplies the
objects those refs name, and only when their bytes hash to the advertised ID.
"""
import os
import re
import subprocess
import time

from staging_cd_common import require
from staging_cd_receipts import git_tag_oid
from staging_cd_versions import REPOSITORY

OID = r"[0-9a-f]{40}"
MAX_TAG = 64 * 1024
# Only final and pending release refs may be named on a fetch command line.
RELEASE_REF = r"refs/tags/[a-z0-9-]+(?:/pending)?/v[0-9]+\.[0-9]+\.[0-9]+"
# One exact-ref fetch per inventory. Any further absent objects use the API.
MAX_FETCH_REFS = 256
REMOTE = f"https://github.com/{REPOSITORY}.git"
# A local Git call never starts with less of the caller's budget than this.
MIN_GIT_SECONDS = 2


class Unreadable(Exception):
    """Local Git output that cannot be trusted; the API reads those objects instead."""


def decode_tag(oid, raw):
    """Return GitHub's three consumed fields for canonical unsigned release tags, else None."""
    if git_tag_oid(raw) != oid:
        # Never trusted, never fatal: the verified API read answers for this ID.
        return None
    # Only decode the simple annotated commit tags emitted by Reservations.
    # GitHub handles any other encoding, signature, header or tag type.
    header, sep, message = raw.partition(b"\n\n")
    fields = re.fullmatch(rb"object ([0-9a-f]{40})\ntype commit\ntag ([a-z0-9./-]+)\n"
                          rb"tagger [^\r\n]+", header)
    if not (sep and fields and message.isascii() and b"-----BEGIN" not in message):
        return None
    return {"sha": oid, "tag": fields[2].decode(), "message": message.decode(),
            "object": {"type": "commit", "sha": fields[1].decode()}}


class LocalTags:
    """Verified annotated tag objects from a job's complete checkout.

    The full checkout usually has every object. Tags written by another job
    may postdate it; with a remote, one bounded fetch names exactly those refs.
    In every other case the remote API stays authoritative.
    """

    def __init__(self, root, *, remote=None, runner=None, deadline=None, clock=time.monotonic):
        self.root, self.remote, self.runner = root, remote, runner
        # An absolute budget on clock(), shared with the job's other reads.
        self.deadline, self.clock = deadline, clock
        self.env = {k: os.environ[k] for k in ("PATH", "LANG", "LC_ALL") if k in os.environ}
        self.env.update(GIT_CONFIG_NOSYSTEM="1", GIT_CONFIG_GLOBAL=os.devnull, GIT_TERMINAL_PROMPT="0",
                        GIT_NO_REPLACE_OBJECTS="1")

    def run(self, args, *, data=None, timeout=30):
        if self.deadline is not None:
            timeout = min(timeout, self.deadline - self.clock())
            if timeout < MIN_GIT_SECONDS:
                raise subprocess.TimeoutExpired(["git", *args], timeout)
        return (self.runner or subprocess.run)(["git", *args], input=data, cwd=self.root, env=self.env,
                                               capture_output=True, timeout=timeout, check=True).stdout

    def __call__(self, path):
        # A local miss always delegates to the live API.
        match = re.fullmatch(r"git/tags/(" + OID + ")", path)
        return self.read([match[1]])[0].get(match[1]) if match else None

    def read(self, oids):
        """Return canonical local tags by ID, and the IDs absent from the checkout.

        An object is returned only when its bytes hash to the advertised ID.
        A mismatch, a failing or slow cat-file, or malformed output is never
        trusted and never fatal: those objects are left to the verified API.
        """
        oids = list(dict.fromkeys(oids))
        for oid in oids:
            require(isinstance(oid, str) and re.fullmatch(OID, oid), "Invalid release tag object ID")
        if not oids:
            return {}, []
        try:
            return self.batch(oids)
        except (OSError, subprocess.SubprocessError, Unreadable):
            return {}, []

    def batch(self, oids):
        git = ["--no-replace-objects", "--no-lazy-fetch", "cat-file"]
        lines = self.run([*git, "--batch-check"], data="".join(oid + "\n" for oid in oids).encode()).splitlines()
        if len(lines) != len(oids):
            raise Unreadable()
        sizes, absent = {}, []
        for oid, line in zip(oids, lines):
            if line == oid.encode() + b" missing":
                absent.append(oid)
                continue
            match = re.fullmatch(oid.encode() + rb" ([a-z]+) ([0-9]{1,10})", line)
            if match is None:
                raise Unreadable()
            if match[1] == b"tag" and int(match[2]) <= MAX_TAG:
                sizes[oid] = int(match[2])
        found = {}
        if sizes:
            raw = self.run([*git, "--batch"], data="".join(oid + "\n" for oid in sizes).encode())
            offset = 0
            for oid, size in sizes.items():
                header = f"{oid} tag {size}\n".encode()
                start = offset + len(header)
                if not (raw.startswith(header, offset) and raw[start + size:start + size + 1] == b"\n"):
                    raise Unreadable()
                value = decode_tag(oid, raw[start:start + size])
                if value is not None:
                    found[oid] = value
                offset = start + size + 1
            if offset != len(raw):
                raise Unreadable()
        return found, absent

    def load(self, refs):
        """Read the objects one live inventory names, fetching absent ones once by exact ref."""
        found, absent = self.read(refs.values())
        absent = set(absent)
        wanted = sorted(ref for ref, oid in refs.items()
                        if oid in absent and isinstance(ref, str) and re.fullmatch(RELEASE_REF, ref))
        if wanted and self.remote is not None and self.fetch(wanted[:MAX_FETCH_REFS]):
            # Content addressing authenticates each object against the live ref's ID.
            # A ref that moved after the inventory read leaves its object absent.
            found.update(self.read(refs[ref] for ref in wanted[:MAX_FETCH_REFS])[0])
        return found

    def fetch(self, refs):
        """Copy exactly these tag refs' objects into the object store, never a ref.

        Without tag following, FETCH_HEAD, replacement objects, credentials or
        maintenance. Failure is not authority: the API reads what is still absent.
        """
        try:
            if self.run(["rev-parse", "--is-shallow-repository"], timeout=10).strip() != b"false":
                # A shallow checkout would download whole release histories.
                return False
            self.run(["-c", "credential.helper=", "-c", "http.extraHeader=", "-c", "gc.auto=0",
                      "-c", "maintenance.auto=false", "-c", "fetch.writeCommitGraph=false",
                      "fetch", "--quiet", "--no-tags", "--no-write-fetch-head", "--no-recurse-submodules",
                      self.remote, *refs], timeout=60)
        except (OSError, subprocess.SubprocessError):
            return False
        return True


def read_local_tag(oid, root):
    """Return GitHub's three consumed fields for a canonical unsigned release tag, or None."""
    return LocalTags(root).read([oid])[0].get(oid)


def local_tag_reader(root, *, deadline=None):
    """Every release lane reads tag objects from its checkout after a live ref read."""
    return LocalTags(root, remote=REMOTE, deadline=deadline)
