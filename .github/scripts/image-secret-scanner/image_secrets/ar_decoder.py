"""Stream a constrained libarchive worker into the existing per-file scanner."""

import json
import os
import signal
import struct
import subprocess
import sys
import threading
from pathlib import Path

from .common import ScanError, archive_path


class MemberStream:
    def __init__(self, stream, remaining):
        self.stream, self.remaining = stream, remaining

    def read(self, count):
        block = self.stream.read(min(count, self.remaining))
        self.remaining -= len(block)
        return block


def exact(stream, count):
    chunks = bytearray()
    while len(chunks) < count:
        block = stream.read(count - len(chunks))
        if not block:
            raise ScanError("nested-ar-decode-failed")
        chunks.extend(block)
    return bytes(chunks)


def stop(process):
    if process.poll() is None:
        try:
            os.killpg(process.pid, signal.SIGKILL)
            process.wait()
        except ProcessLookupError:
            pass


def decode_ar(stage, physical, location, depth, member_limit, child_location):
    remaining = stage.budget.max_bytes - stage.budget.bytes
    entries = stage.budget.max_entries - stage.budget.entries
    env = {key: value for key, value in os.environ.items() if key in {"PATH", "LANG", "LC_ALL"}}
    env["PYTHONDONTWRITEBYTECODE"] = "1"
    command = [sys.executable, "-B", "-m", "image_secrets.ar_worker", str(physical.resolve()),
               str(remaining), str(member_limit), str(entries)]
    try:
        process = subprocess.Popen(command, cwd=Path(__file__).parent.parent, env=env,
                                   stdout=subprocess.PIPE, stderr=subprocess.DEVNULL, start_new_session=True)
    except OSError as error:
        raise ScanError("ar-decoder-unavailable") from error
    expired = threading.Event()

    def timeout():
        expired.set()
        stop(process)

    timer = threading.Timer(stage.timeout, timeout)
    timer.start()
    try:
        ordinal = 0
        while True:
            length = struct.unpack(">Q", exact(process.stdout, 8))[0]
            if length == 0:
                break
            if length > 65536:
                raise ScanError("nested-ar-decode-failed")
            row = json.loads(exact(process.stdout, length))
            if (not isinstance(row, dict) or set(row) != {"ordinal", "path", "size"}
                    or type(row["ordinal"]) is not int or row["ordinal"] != ordinal
                    or type(row["size"]) is not int or not 0 <= row["size"] <= member_limit):
                raise ScanError("nested-ar-decode-failed")
            name = archive_path(row["path"])
            stage.budget.entry()
            child = child_location(location, ordinal, name)
            stage.add(json.dumps({"path": name}).encode(), child)
            member = MemberStream(process.stdout, row["size"])
            stage.file(member, row["size"], child, name, depth + 1)
            if member.remaining:
                raise ScanError("nested-ar-decode-failed")
            ordinal += 1
        if process.stdout.read(1) or process.wait() != 0:
            raise ScanError("nested-ar-decode-failed")
    except (OSError, ValueError, TypeError, ScanError) as error:
        if expired.is_set():
            raise ScanError("nested-ar-timeout") from error
        raise
    finally:
        timer.cancel()
        timer.join()
        stop(process)
        process.stdout.close()
        process.wait()
    if expired.is_set():
        raise ScanError("nested-ar-timeout")
