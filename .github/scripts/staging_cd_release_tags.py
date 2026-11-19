"""Resolve immutable tag objects from the release checkout after a live ref read."""
import hashlib
import os
import re
import subprocess
from functools import lru_cache

from staging_cd_common import require

OID = r"[0-9a-f]{40}"
MAX_TAG = 64 * 1024


def read_local_tag(oid, root):
    """Return GitHub's three consumed fields for canonical unsigned release tags."""
    require(re.fullmatch(OID, oid), "Invalid release tag object ID")
    # The full checkout usually has the object. Fresh refs and tags written by
    # another job may postdate checkout. In either case the remote API is authoritative.
    git = ["git", "--no-replace-objects", "--no-lazy-fetch", "cat-file"]
    env = {k: os.environ[k] for k in ("PATH", "LANG", "LC_ALL") if k in os.environ}
    env.update(GIT_CONFIG_NOSYSTEM="1", GIT_CONFIG_GLOBAL=os.devnull, GIT_TERMINAL_PROMPT="0")
    check = subprocess.run([*git, "--batch-check"], input=oid.encode() + b"\n", cwd=root, env=env,
                           capture_output=True, timeout=5, check=True).stdout.strip()
    match = re.fullmatch(oid.encode() + rb" tag ([0-9]{1,10})", check)
    if match is None or int(match[1]) > MAX_TAG:
        return None
    raw = subprocess.run([*git, "tag", oid], cwd=root, env=env, capture_output=True,
                         timeout=5, check=True).stdout
    require(len(raw) == int(match[1]) and
            hashlib.sha1(b"tag " + str(len(raw)).encode() + b"\0" + raw).hexdigest() == oid,
            "Local tag object differs from advertised ID")
    # Only decode the simple annotated commit tags emitted by Reservations.
    # GitHub handles any other encoding, signature, header or tag type.
    header, sep, message = raw.partition(b"\n\n")
    fields = re.fullmatch(rb"object ([0-9a-f]{40})\ntype commit\ntag ([a-z0-9./-]+)\n"
                          rb"tagger [^\r\n]+", header)
    if not (sep and fields and message.isascii() and b"-----BEGIN" not in message):
        return None
    return {"sha": oid, "tag": fields[2].decode(), "message": message.decode(),
            "object": {"type": "commit", "sha": fields[1].decode()}}


def local_tag_reader(component, root):
    if component != "web":
        return None

    @lru_cache(maxsize=512)
    def by_oid(oid):
        return read_local_tag(oid, root)

    def read(path):
        # Every mutable ref is still read through GitHub. A local miss always
        # delegates to the live API, including tags created after checkout.
        match = re.fullmatch(r"git/tags/(" + OID + ")", path)
        return by_oid(match[1]) if match else None

    return read
