"""Resolve immutable tag objects from the release checkout after a live ref read.

Mutable refs are always read live from GitHub. This module only supplies the
objects those refs name, and only when their bytes hash to the advertised ID.
"""
import os
import re
import subprocess

from staging_cd_common import require
from staging_cd_receipts import git_tag_oid

OID = r"[0-9a-f]{40}"
MAX_TAG = 64 * 1024


def decode_tag(oid, raw):
    """Return GitHub's three consumed fields for canonical unsigned release tags."""
    require(git_tag_oid(raw) == oid, "Local tag object differs from advertised ID")
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
    may postdate it; in that case the remote API stays authoritative.
    """

    def __init__(self, root, *, runner=None):
        self.root, self.runner = root, runner
        self.env = {k: os.environ[k] for k in ("PATH", "LANG", "LC_ALL") if k in os.environ}
        self.env.update(GIT_CONFIG_NOSYSTEM="1", GIT_CONFIG_GLOBAL=os.devnull, GIT_TERMINAL_PROMPT="0",
                        GIT_NO_REPLACE_OBJECTS="1")

    def run(self, args, *, data=None, timeout=30):
        return (self.runner or subprocess.run)(["git", *args], input=data, cwd=self.root, env=self.env,
                                               capture_output=True, timeout=timeout, check=True).stdout

    def __call__(self, path):
        # A local miss always delegates to the live API.
        match = re.fullmatch(r"git/tags/(" + OID + ")", path)
        return self.read([match[1]])[0].get(match[1]) if match else None

    def read(self, oids):
        """Return canonical local tags by ID, and the IDs absent from the checkout."""
        oids = list(dict.fromkeys(oids))
        for oid in oids:
            require(isinstance(oid, str) and re.fullmatch(OID, oid), "Invalid release tag object ID")
        if not oids:
            return {}, []
        git = ["--no-replace-objects", "--no-lazy-fetch", "cat-file"]
        lines = self.run([*git, "--batch-check"], data="".join(oid + "\n" for oid in oids).encode()).splitlines()
        require(len(lines) == len(oids), "Local tag object differs from advertised ID")
        sizes, absent = {}, []
        for oid, line in zip(oids, lines):
            if line == oid.encode() + b" missing":
                absent.append(oid)
                continue
            match = re.fullmatch(oid.encode() + rb" ([a-z]+) ([0-9]{1,10})", line)
            require(match is not None, "Local tag object differs from advertised ID")
            if match[1] == b"tag" and int(match[2]) <= MAX_TAG:
                sizes[oid] = int(match[2])
        found = {}
        if sizes:
            raw = self.run([*git, "--batch"], data="".join(oid + "\n" for oid in sizes).encode())
            offset = 0
            for oid, size in sizes.items():
                header = f"{oid} tag {size}\n".encode()
                start = offset + len(header)
                require(raw.startswith(header, offset) and raw[start + size:start + size + 1] == b"\n",
                        "Local tag object differs from advertised ID")
                value = decode_tag(oid, raw[start:start + size])
                if value is not None:
                    found[oid] = value
                offset = start + size + 1
            require(offset == len(raw), "Local tag object differs from advertised ID")
        return found, absent

    def load(self, refs):
        """Read every object one live inventory names in two local Git calls."""
        return self.read(refs.values())[0]


def read_local_tag(oid, root):
    """Return GitHub's three consumed fields for a canonical unsigned release tag, or None."""
    return LocalTags(root).read([oid])[0].get(oid)


def local_tag_reader(root):
    """Every release lane reads tag objects from its checkout after a live ref read."""
    return LocalTags(root)
