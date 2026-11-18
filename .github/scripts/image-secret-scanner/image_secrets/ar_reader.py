"""Read static-library members with the platform archive tool, never execute them."""

import os
import subprocess
import tempfile
import threading
from pathlib import Path

from .common import ScanError, archive_path, copy_stream


def run_ar(operation, source, destination, limit, timeout, member=None):
    # Neither image-controlled names nor tool diagnostics may reach the terminal.
    args = ["ar", operation, str(source)]
    if member is not None:
        args.append(member)
    env = {key: value for key, value in os.environ.items() if key in ("PATH", "HOME")}
    env["LC_ALL"] = "C"
    with tempfile.TemporaryFile() as errors:
        try:
            process = subprocess.Popen(args, stdout=subprocess.PIPE, stderr=errors, env=env)
        except OSError as error:
            raise ScanError("ar-unavailable") from error
        timer = threading.Timer(timeout, process.kill)
        timer.start()
        try:
            copy_stream(process.stdout, destination, limit)
            code = process.wait(timeout=timeout)
            errors.seek(0)
            if code != 0 or errors.read(1):
                raise ScanError("ar-member-read-failed")
        except subprocess.TimeoutExpired as error:
            raise ScanError("ar-read-timeout") from error
        finally:
            timer.cancel()
            timer.join()
            process.stdout.close()
            if process.poll() is None:
                process.kill()
            process.wait()


def members(source, directory, budget, timeout):
    listing = directory / "ar-members.txt"
    try:
        remaining = budget.max_entries - budget.entries
        run_ar("t", source, listing, min(16 * 1024 * 1024, max(0, remaining) * 4096), timeout)
        data = listing.read_bytes()
        if data and not data.endswith(b"\n"):
            raise ScanError("invalid-ar-member-list")
        try:
            names = data.decode("utf-8").splitlines()
        except UnicodeError as error:
            raise ScanError("invalid-ar-member-name") from error
        if len(names) > remaining:
            raise ScanError("entry-limit")
        seen = set()
        for name in names:
            # ar's named extraction is ambiguous for duplicates. Refuse rather
            # than accidentally scanning only the first member of that name.
            if not name or name.startswith("-") or any(ord(char) < 32 or ord(char) == 127 for char in name):
                raise ScanError("invalid-ar-member-name")
            normalized = archive_path(name)
            if normalized in seen:
                raise ScanError("ambiguous-ar-member")
            seen.add(normalized)
            yield name, normalized
    finally:
        listing.unlink(missing_ok=True)


def decode_ar(stage, physical, location, depth, child_location):
    # Give each nested archive its own scratch directory; no member paths are
    # ever used as extraction destinations on the host filesystem.
    with tempfile.TemporaryDirectory(prefix="ar-decode-", dir=stage.root.parent) as temporary:
        directory = Path(temporary)
        for index, (member, name) in enumerate(
            members(physical, directory, stage.budget, stage.backend.timeout)
        ):
            stage.budget.entry()
            child = child_location(location, index, name)
            stage.add(name.encode("utf-8"), child)
            target = directory / "member.bin"
            try:
                run_ar(
                    "p",
                    physical,
                    target,
                    stage.budget.max_bytes - stage.budget.bytes,
                    stage.backend.timeout,
                    member,
                )
                with target.open("rb") as incoming:
                    stage.file(incoming, target.stat().st_size, child, name, depth + 1)
            finally:
                target.unlink(missing_ok=True)
