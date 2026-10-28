"""Bounded, content-free primitives; concatenate before recovery.py for exec.

Only the admitted recovery entry point writes, and only its own journal. History
and /proc reads never write to provider stdin, SQLite, configuration or logs.
"""
import contextlib
import datetime
import fcntl
import hashlib
import json
import os
import re
import signal
import socket
import stat
import struct
import sys
import time
import uuid

LIMIT = 65536
PROC = "/proc"
HOME = "/home/cma"
STATE = "/var/lib/cma-runtime"
DEADLINE = float("inf")
UUID = re.compile(r"[0-9a-f]{8}(?:-[0-9a-f]{4}){3}-[0-9a-f]{12}\Z")


class Refused(Exception):
    pass


def require(ok, code):
    if not ok:
        raise Refused(code)
    require_time()


def require_time():
    if time.monotonic() >= DEADLINE:
        raise Refused("deadline")


def encode(value):
    return json.dumps(value, sort_keys=True, separators=(",", ":"), ensure_ascii=True).encode()


def sha(value):
    return hashlib.sha256(value).hexdigest()


@contextlib.contextmanager
def directory(path):
    """Walk each component through retained descriptors, rejecting symlinks."""
    require(os.path.isabs(path) and os.path.normpath(path) == path, "unsafe_path")
    fd = os.open("/", os.O_RDONLY | os.O_DIRECTORY | os.O_CLOEXEC)
    try:
        for name in path.split("/")[1:]:
            if name:
                nxt = os.open(name, os.O_RDONLY | os.O_DIRECTORY | os.O_NOFOLLOW | os.O_CLOEXEC, dir_fd=fd)
                os.close(fd)
                fd = nxt
        yield fd
    finally:
        os.close(fd)


@contextlib.contextmanager
def regular(path, flags=os.O_RDONLY, mode=0o600):
    with directory(os.path.dirname(path)) as parent:
        fd = os.open(os.path.basename(path), flags | os.O_NOFOLLOW | os.O_NONBLOCK | os.O_CLOEXEC, mode, dir_fd=parent)
        try:
            require(stat.S_ISREG(os.fstat(fd).st_mode), "unsafe_file")
            yield fd
        finally:
            os.close(fd)


def read(path, maximum=LIMIT, proc=False):
    if proc:
        with open(path, "rb", buffering=0) as file:
            value = file.read(maximum + 1)
    else:
        with regular(path) as fd:
            value = os.read(fd, maximum + 1)
    require(len(value) <= maximum, "size_limit")
    return value


def digest_file(path, proc=False):
    # Native is ~150MiB; stream without retaining bytes or invoking the binary.
    h, total = hashlib.sha256(), 0
    manager = open(path, "rb", buffering=0) if proc else regular(path)
    with manager as file:
        while True:
            chunk = file.read(1048576) if proc else os.read(file, 1048576)
            if not chunk:
                return h.hexdigest()
            h.update(chunk)
            total += len(chunk)
            require(total <= 512 * 1048576, "binary_limit")


def proc_stat(pid):
    raw = read(f"{PROC}/{pid}/stat", 4096, True).decode()
    tail = raw[raw.rindex(")") + 2:].split()
    return {"pid": pid, "start": int(tail[19]), "ppid": int(tail[1]), "sid": int(tail[3]), "state": tail[0]}


def inventory():
    names = os.listdir(PROC)
    require(len(names) <= 8192, "process_limit")
    result = {}
    for name in names:
        if name.isdigit():
            try:
                item = proc_stat(int(name))
                if item["state"] != "Z":
                    result[item["pid"]] = item
            except FileNotFoundError:
                pass
    require(len(result) <= 512, "process_limit")
    return result


def proc_identity(pid):
    before = proc_stat(pid)
    root = f"{PROC}/{pid}"
    command = read(root + "/cmdline", 32768, True)
    result = {k: before[k] for k in ("pid", "start", "ppid", "sid")}
    result.update(commandSha256=sha(command), exeSha256=digest_file(root + "/exe", True))
    require(proc_stat(pid) == before, "process_changed")
    return result


def environment(pid):
    return dict(item.split(b"=", 1) for item in read(f"{PROC}/{pid}/environ", 262144, True).split(b"\0") if b"=" in item)


def same_process(item, current):
    return current is not None and current["start"] == item["start"]


def descendants(pids, processes):
    result = set(pids)
    while True:
        added = {pid for pid, p in processes.items() if p["ppid"] in result} - result
        if not added:
            return result
        result |= added


def unix_peer(inode):
    """Read Linux UNIX_DIAG_PEER; Node's libuv 'pipe' is usually a socketpair."""
    request = struct.pack("=BBHIIIII", socket.AF_UNIX, 0, 0, 0xffffffff, 0, 4, 0xffffffff, 0xffffffff)
    with socket.socket(socket.AF_NETLINK, socket.SOCK_RAW, 4) as sock:
        sock.settimeout(0.5)
        sock.send(struct.pack("=IHHII", 16 + len(request), 20, 0x301, 1, 0) + request)
        total, peer = 0, None
        while True:
            data = sock.recv(65536)
            total += len(data)
            require(total <= 4 * 1048576, "socket_limit")
            offset = 0
            while offset < len(data):
                length, kind, _, _, _ = struct.unpack_from("=IHHII", data, offset)
                require(length >= 16 and offset + length <= len(data), "socket_evidence")
                if kind == 3:
                    require(peer is not None, "socket_peer_missing")
                    return peer
                if kind == 2 and length >= 20 and struct.unpack_from("=i", data, offset + 16)[0] == -2:
                    raise Refused("socket_diagnostic_unsupported")
                require(kind == 20 and length >= 32, "socket_evidence")
                found = struct.unpack_from("=I", data, offset + 20)[0]
                pos = offset + 32
                while pos + 4 <= offset + length:
                    size, attr = struct.unpack_from("=HH", data, pos)
                    require(size >= 4 and pos + size <= offset + length, "socket_evidence")
                    if found == inode and attr == 2 and size == 8:
                        peer = struct.unpack_from("=I", data, pos + 4)[0]
                    pos += (size + 3) & ~3
                offset += (length + 3) & ~3


def socket_fds(processes):
    result = {}
    for pid in processes:
        try:
            entries = os.listdir(f"{PROC}/{pid}/fd")
            require(len(entries) <= 4096, "fd_limit")
            for entry in entries:
                try:
                    target = os.readlink(f"{PROC}/{pid}/fd/{entry}")
                    if target.startswith("socket:["):
                        result[(pid, int(entry))] = target
                except FileNotFoundError:
                    pass
        except FileNotFoundError:
            pass
    return result


def source_stdio(native, launcher, bridge, processes):
    """Pinned ACP spawns exactly one child with three Node 'pipe' socketpairs.

    Pinned npm inherits all three unchanged. ACP sends no descriptors or other
    sockets to its child. All three exclusive parent endpoints close together on
    ACP exit: identifying which of them is stdin's peer is unnecessary. This mode
    needs every endpoint exclusive, unbound and connected; never inode adjacency.
    """
    require(launcher is not None, "socket_diagnostic_unsupported")
    fds = socket_fds(processes)
    parent = sorted({v for (pid, _), v in fds.items() if pid == bridge})
    child = [fds.get((native, fd)) for fd in range(3)]
    require(len(parent) == 3 and None not in child and len(set(parent + child)) == 6, "stdio_shape")
    require(child == [fds.get((launcher, fd)) for fd in range(3)], "stdio_inheritance")
    for endpoint in parent + child:
        owners = {pid for (pid, _), v in fds.items() if v == endpoint}
        require(owners == ({bridge} if endpoint in parent else {launcher, native}), "stdio_shared")
    rows = [row.split() for row in read(PROC + "/net/unix", 1048576, True).decode().splitlines()[1:]]
    for endpoint in parent + child:
        matching = [row for row in rows if len(row) >= 7 and row[6] == endpoint[8:-1]]
        require(len(matching) == 1 and len(matching[0]) == 7 and matching[0][3:6] == ["00000000", "0001", "03"], "stdio_socket_kind")
    require(socket_fds(processes) == fds, "stdio_changed")
    return {"mode": "pinned_source_stdio", "reader": child[0], "parentEndpoints": parent, "childEndpoints": child}


def stdin_owner(native, bridge, processes, launcher=None):
    pipe = os.readlink(f"{PROC}/{native}/fd/0")
    match = re.fullmatch(r"(pipe|socket):\[([0-9]+)\]", pipe)
    require(match is not None, "stdin_not_pipe")
    try:
        endpoint = pipe if match[1] == "pipe" else "socket:[" + str(unix_peer(int(match[2]))) + "]"
    except Refused as error:
        if str(error) != "socket_diagnostic_unsupported":
            raise
        return source_stdio(native, launcher, bridge, processes)
    owners = set()
    for pid in processes:
        try:
            entries = os.listdir(f"{PROC}/{pid}/fd")
            require(len(entries) <= 4096, "fd_limit")
            for entry in entries:
                try:
                    if os.readlink(f"{PROC}/{pid}/fd/{entry}") != endpoint:
                        continue
                    info = read(f"{PROC}/{pid}/fdinfo/{entry}", 4096, True).decode()
                    flags = int(re.search(r"^flags:\s+([0-7]+)$", info, re.M)[1], 8)
                    if flags & os.O_ACCMODE != os.O_RDONLY:
                        owners.add(pid)
                except FileNotFoundError:
                    pass
        except FileNotFoundError:
            pass
    require(owners == {bridge}, "stdin_owner_mismatch")
    return {"mode": "kernel_peer", "reader": pipe, "writer": endpoint}


def history_file(path, relative, old=None):
    with regular(path) as fd:
        meta = os.fstat(fd)
        require(0 < meta.st_size <= 512 * 1048576, "history_limit")
        if old:
            require([meta.st_dev, meta.st_ino] == old["fileIdentity"] and meta.st_size >= old["bytes"], "history_replaced")
        full, prefix, count, seen, active, last = hashlib.sha256(), hashlib.sha256(), 0, None, {}, None
        ordinal, terminal = 0, None
        with os.fdopen(os.dup(fd), "rb") as file:
            while count < meta.st_size:
                line = file.readline(min(8 * 1048576 + 1, meta.st_size - count))
                require(line.endswith(b"\n") and len(line) <= 8 * 1048576, "history_incomplete")
                full.update(line)
                if old and count < old["bytes"]:
                    prefix.update(line[:old["bytes"] - count])
                count += len(line)
                ordinal += 1
                value = json.loads(line)
                require(isinstance(value, dict), "history_invalid")
                payload = value.get("payload", {})
                if value.get("type") == "session_meta":
                    require(seen is None and UUID.fullmatch(payload.get("id", "")), "history_identity")
                    seen = payload["id"]
                if value.get("type") == "event_msg" and isinstance(payload, dict):
                    kind, turn = payload.get("type"), payload.get("turn_id")
                    if kind in ("task_started", "turn_started", "task_complete", "turn_complete", "turn_aborted"):
                        require(isinstance(turn, str) and UUID.fullmatch(turn), "turn_identity")
                        try:
                            timestamp = value["timestamp"]
                            require(isinstance(timestamp, str) and timestamp.endswith("Z"), "turn_timestamp")
                            at = int(datetime.datetime.fromisoformat(timestamp[:-1] + "+00:00").timestamp() * 1000)
                        except (KeyError, ValueError, TypeError, Refused):
                            at = None
                        if kind in ("task_started", "turn_started"):
                            active[turn] = {"at": at, "ordinal": ordinal}
                        else:
                            active.pop(turn, None)
                            terminal = {"at": at, "ordinal": ordinal}
                        last = {"turnId": turn, "type": kind}
                require(len(active) <= 128, "turn_limit")
        require(seen is not None, "history_identity")
        require(not old or (prefix.hexdigest() == old["sha256"] and seen == old["conversationId"]), "history_prefix_changed")
        return {"path": relative, "fileIdentity": [meta.st_dev, meta.st_ino], "conversationId": seen,
                "bytes": count, "sha256": full.hexdigest(), "openTurns": sorted(active), "lastTurn": last,
                "openTurnStarts": active, "lastTerminal": terminal}


def histories(agent, previous=None):
    home = HOME + "/.cma/agents/" + agent["agentId"] + "/home"
    old = {x["path"]: x for x in previous or []}
    result, visited = [], 0
    with directory(home):
        pass
    for base in ("sessions", "archived_sessions"):
        top = home + "/" + base
        if not os.path.lexists(top):
            continue
        for root, dirs, files in os.walk(top, followlinks=False):
            with directory(root):
                pass
            visited += len(dirs) + len(files) + 1
            require(visited <= 8192, "history_limit")
            require(not any(os.path.islink(root + "/" + name) for name in dirs + files), "history_symlink")
            for name in sorted(files):
                if name.endswith(".jsonl"):
                    path = root + "/" + name
                    relative = os.path.relpath(path, home)
                    require(len(relative) < 256 and re.fullmatch(r"[a-zA-Z0-9/_.:-]+", relative), "history_path")
                    result.append(history_file(path, relative, old.get(relative)))
                    require(len(result) <= 128, "history_limit")
    result.sort(key=lambda x: x["path"])
    require(set(old) <= {x["path"] for x in result}, "history_missing")
    ids = [x["conversationId"] for x in result]
    require(len(ids) == len(set(ids)) and agent["conversationId"] in ids, "history_identity")
    return result
