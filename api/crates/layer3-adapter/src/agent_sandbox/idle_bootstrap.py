"""Read-only observation for one pinned legacy bootstrap, never a stop command.

The pinned binary creates a permanent child-reaper thread before constructing
its supervisor. Bootstrap argv and a lack of child processes alone are not proof.
No request is sent to the assignment socket and no tenant file is opened.
"""
import hashlib
import json
import os
from pathlib import Path
import re
import time

BINARY_SHA256 = "5a39d8c147fb9061e85831aa5fc7f42c405374719c4bc6d040e94dd2f997939b"
BINARY = "/usr/local/bin/layer4-runtime"
SOCKET = "/var/lib/cma-runtime/assignment.sock"


class Refused(Exception):
    pass


def require(condition):
    if not condition:
        raise Refused()


def bounded(path, limit=65536):
    with path.open("rb") as source:
        value = source.read(limit + 1)
    require(len(value) <= limit)
    return value


def process(path):
    raw = bounded(path / "stat").decode()
    fields = raw[raw.rindex(")") + 1:].split()
    require(len(fields) >= 20)
    require(fields[0] in ("S", "R", "I"))
    return {"parent": int(fields[1]), "start": int(fields[19])}


def observe(proc, own_pid):
    pids = {int(entry.name) for entry in proc.iterdir() if entry.name.isdecimal()}
    require(pids == {1, own_pid})
    root = proc / "1"
    identity = process(root)
    require(identity["parent"] == 0 and identity["start"] > 0)
    require(os.readlink(root / "exe") == BINARY)
    require(bounded(root / "cmdline") == (BINARY + "\0--wait-for-assignment\0").encode())
    threads = {}
    for task in (root / "task").iterdir():
        require(task.name.isdecimal())
        name = bounded(task / "comm", 64).decode().strip()
        require(name == ("layer4-runtime" if task.name == "1" else "tokio-rt-worker"))
        threads[task.name] = process(task)
    require("1" in threads and 2 <= len(threads) <= 257)
    descriptors = {}
    sockets = set()
    for descriptor in (root / "fd").iterdir():
        require(descriptor.name.isdecimal())
        target = os.readlink(descriptor)
        descriptors[descriptor.name] = target
        if int(descriptor.name) <= 2:
            require(re.fullmatch(r"pipe:\[\d+\]", target) is not None)
        elif match := re.fullmatch(r"socket:\[(\d+)\]", target):
            sockets.add(match.group(1))
        else:
            require(target in ("anon_inode:[eventpoll]", "anon_inode:[eventfd]"))
    require(all(str(fd) in descriptors for fd in range(3)))
    require(6 <= len(descriptors) <= 32 and len(sockets) == 3)
    rows = {}
    for line in bounded(root / "net/unix").decode().splitlines()[1:]:
        fields = line.split()
        require(len(fields) >= 7)
        if fields[6] in sockets:
            require(fields[6] not in rows)
            rows[fields[6]] = fields[3:6] + fields[7:]
    require(set(rows) == sockets)
    require(list(rows.values()).count(["00010000", "0001", "01", SOCKET]) == 1)
    require(list(rows.values()).count(["00000000", "0001", "03"]) == 2)
    # An assigned supervisor opens additional sockets or descriptors. Every
    # descriptor is accounted for above; do not inspect unrelated network users.
    require(pids == {int(p.name) for p in proc.iterdir() if p.name.isdecimal()})
    require(process(root) == identity)
    return {"identity": identity, "threads": threads, "fds": descriptors, "sockets": rows}


def inspect():
    proc = Path("/proc")
    binary = proc / "1/exe"
    before = binary.stat()
    digest = hashlib.sha256()
    with binary.open("rb") as source:
        for block in iter(lambda: source.read(1024 * 1024), b""):
            digest.update(block)
    after = binary.stat()
    require((before.st_dev, before.st_ino, before.st_size, before.st_mtime_ns)
            == (after.st_dev, after.st_ino, after.st_size, after.st_mtime_ns))
    require(digest.hexdigest() == BINARY_SHA256)
    first = observe(proc, os.getpid())
    time.sleep(0.05)
    require(observe(proc, os.getpid()) == first)
    encoded = json.dumps(first, sort_keys=True, separators=(",", ":")).encode()
    return {"version": 1, "pid_start_ticks": first["identity"]["start"],
            "binary_sha256": BINARY_SHA256,
            "process_sha256": hashlib.sha256(encoded).hexdigest()}


if __name__ == "__main__":
    try:
        print(json.dumps({"status": "idle", "proof": inspect()}))
    except (Refused, OSError, ValueError, UnicodeError, IndexError):
        # No commands, environment, paths, socket peers or tenant data escape.
        print(json.dumps({"status": "refused"}))
