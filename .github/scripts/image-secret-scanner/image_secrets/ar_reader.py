"""Read static-library members with the platform archive tool, never execute them."""

import io
import os
import subprocess
import tempfile
import threading
from pathlib import Path

from .ar_inventory import inventory
from .common import ScanError, archive_path, copy_stream, digest_file


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
    physical = inventory(source, budget.max_entries - budget.entries)
    listing = directory / "ar-members.txt"
    try:
        run_ar("t", source, listing, min(16 * 1024 * 1024, len(physical) * 4096), timeout)
        data = listing.read_bytes()
        if data and not data.endswith(b"\n"):
            raise ScanError("invalid-ar-member-list")
        try:
            names = data.decode("utf-8").splitlines()
        except UnicodeError as error:
            raise ScanError("invalid-ar-member-name") from error
        metadata = {member.name for member in physical if member.metadata}
        ordinary = [member.name for member in physical if not member.metadata]
        listed = [archive_path(name) for name in names if name not in metadata]
        if listed != ordinary:
            raise ScanError("incomplete-ar-member-list")
        yield from physical
    finally:
        listing.unlink(missing_ok=True)


def copy_member(source, member, target):
    with source.open("rb") as incoming, target.open("xb") as outgoing:
        incoming.seek(member.start)
        remaining = member.size
        while remaining:
            chunk = incoming.read(min(1024 * 1024, remaining))
            if not chunk:
                raise ScanError("truncated-ar-member")
            outgoing.write(chunk)
            remaining -= len(chunk)


def decode_ar(stage, physical, location, depth, child_location):
    # Every physical member is accounted for, including metadata hidden by ar t.
    # Native extraction must agree byte-for-byte with the structural inventory.
    with tempfile.TemporaryDirectory(prefix="ar-decode-", dir=stage.root.parent) as temporary:
        directory = Path(temporary)
        for index, member in enumerate(members(physical, directory, stage.budget, stage.backend.timeout)):
            stage.budget.entry()
            name = "ar-metadata-" + str(index) if member.metadata else member.name
            child = child_location(location, index, name)
            stage.add(member.name.encode("utf-8"), child)
            remaining = stage.budget.max_bytes - stage.budget.bytes
            if member.size > remaining:
                raise ScanError("decoded-byte-limit")
            target, native = directory / "member.bin", directory / "native.bin"
            try:
                copy_member(physical, member, target)
                if not member.metadata:
                    run_ar("p", physical, native, remaining, stage.backend.timeout, member.name)
                    if native.stat().st_size != member.size or digest_file(native) != digest_file(target):
                        raise ScanError("ar-member-reader-disagrees")
                with target.open("rb") as incoming:
                    stage.file(incoming, member.size, child, name, depth + 1)
                # Length/index prefixes must not hide compressed symbol/name
                # strings from the recursive decoder's start-of-input checks.
                for number, data in enumerate(member.fragments):
                    stage.budget.entry()
                    part = child_location(child, number, "metadata-string")
                    stage.file(io.BytesIO(data), len(data), part, None, depth + 1)
            finally:
                target.unlink(missing_ok=True)
                native.unlink(missing_ok=True)
