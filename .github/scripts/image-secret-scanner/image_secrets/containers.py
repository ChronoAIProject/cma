"""Magic-based nested container decoding, independent of names and Gitleaks dispatch."""

import gzip
import json
import lzma
import subprocess
import tarfile
import threading
import zipfile

from .ar_decoder import decode_ar
from .common import MAX_NESTED_MEMBER_BYTES, ScanError, archive_path, copy_stream
from .container_limits import BoundedReader, zip_directory
from .tar_trailer import verify_trailer
from .xz_stream import decode_xz
from .zip_framing import leading_prefix, member_ends
from .zip_probe import prefixed_zip
from .zip_stream import verified_member

MAX_DEPTH = 3


def identify(path):
    with path.open("rb") as stream:
        header = stream.read(512)
    # Executable bytes can contain incidental ZIP directory signatures. They
    # remain raw scanned content; identifying them never executes the binary.
    if header.startswith(b"\x7fELF"):
        return None
    for magic, kind in [
        (b"!<arch>\n", "ar"),
        (b"PK\x03\x04", "zip"),
        (b"PK\x05\x06", "zip"),
        (b"PK\x07\x08", "zip"),
        (b"\x1f\x8b", "gzip"),
        (b"\xfd7zXZ\x00", "xz"),
        (b"\x28\xb5\x2f\xfd", "zstd"),
    ]:
        if header.startswith(magic):
            return kind
    if len(header) >= 4 and 0x50 <= header[0] <= 0x5F and header[1:4] == b"\x2a\x4d\x18":
        raise ScanError("unsupported-zstd-skippable-frame")
    if header.startswith((b"Rar!\x1a\x07", b"7z\xbc\xaf\x27\x1c", b"!<thin>\n", b"BZh",
                          b"hsqs", b"sqsh", b"070701", b"070702", b"070707", b"\xed\xab\xee\xdb",
                          b"MSCF", b"xar!", b"\x04\x22\x4d\x18", b"\x02\x21\x4c\x18",
                          b"LZIP", b"\x1f\x9d")):
        raise ScanError("unsupported-nested-container")
    if header[257:262] == b"ustar":
        return "tar"
    # Legacy tar has no magic string; its 512-byte header checksum is authoritative.
    if len(header) == 512:
        if header == bytes(512):
            return "tar"
        try:
            tarfile.TarInfo.frombuf(header, "utf-8", "surrogateescape")
        except tarfile.HeaderError:
            pass
        else:
            return "tar"
    # Include self-extracting/prefixed ZIPs whose central directory identifies the format.
    if prefixed_zip(path):
        return "zip"
    return None


def child_location(parent, index, path):
    return dict(
        parent,
        path=parent["path"] + "!" + path,
        member=(parent.get("member", "") + "/" + str(index)).lstrip("/"),
    )


def inspect_nested(stage, physical, location, depth):
    kind = identify(physical)
    if kind is None:
        return
    if depth >= MAX_DEPTH:
        raise ScanError("nested-archive-depth-limit")
    if physical.stat().st_size > MAX_NESTED_MEMBER_BYTES:
        raise ScanError("nested-container-size-limit")
    try:
        decode_nested(stage, physical, location, depth, kind)
    except (
        OSError,
        EOFError,
        ValueError,
        NotImplementedError,
        zipfile.BadZipFile,
        tarfile.TarError,
        lzma.LZMAError,
    ) as error:
        # Decoder messages can contain private names/bytes; expose only the format.
        raise ScanError("nested-" + kind + "-decode-failed") from error


def decode_nested(stage, physical, location, depth, kind):
    if kind == "ar":
        decode_ar(stage, physical, location, depth, MAX_NESTED_MEMBER_BYTES, child_location)
    elif kind == "zip":
        directory = zip_directory(physical, stage.budget.max_entries - stage.budget.entries)
        with zipfile.ZipFile(physical) as archive:
            ends = member_ends(archive.infolist(), directory)
            prefix = leading_prefix(physical, min(ends, default=directory))
            if prefix:
                stage.budget.entry()
                stage.add(prefix, child_location(location, "prefix", "@zip-prefix"))
            for index, member in enumerate(archive.infolist()):
                stage.budget.entry()
                name = archive_path(member.filename)
                child = child_location(location, index, name)
                if member.flag_bits & 1:
                    raise ScanError("encrypted-nested-archive")
                stage.add(json.dumps({"path": name}).encode(), child)
                remaining = stage.budget.max_bytes - stage.budget.bytes
                with verified_member(physical, member, ends[member.header_offset], remaining) as stream:
                    if member.is_dir():
                        if member.file_size:
                            raise ScanError("nested-zip-directory-payload")
                        stage.inspect_path(child, name)
                    else:
                        stage.file(stream, member.file_size, child, name, depth + 1)
    elif kind == "tar":
        with physical.open("rb") as source, tarfile.open(fileobj=BoundedReader(source), mode="r:") as archive:
            for index, member in enumerate(archive):
                stage.budget.entry()
                name = archive_path(member.name)
                child = child_location(location, index, name)
                if member.issparse():
                    raise ScanError("sparse-nested-archive")
                stage.add(
                    json.dumps({"path": name, "link": member.linkname, "pax": member.pax_headers}).encode(),
                    child,
                )
                if member.isfile():
                    with archive.extractfile(member) as stream:
                        stage.file(stream, member.size, child, name, depth + 1)
                elif member.isdir() or member.issym() or member.islnk() or member.isdev():
                    stage.inspect_path(child, name)
                else:
                    raise ScanError("unsupported-nested-entry")
            verify_trailer(archive, MAX_NESTED_MEMBER_BYTES)
    else:
        unpacked = physical.with_suffix(".decoded")
        try:
            stage.budget.entry()
            remaining = min(MAX_NESTED_MEMBER_BYTES, stage.budget.max_bytes - stage.budget.bytes)
            if kind == "zstd":
                decode_zstd(physical, unpacked, remaining, stage.backend.timeout)
            elif kind == "xz":
                decode_xz(physical, unpacked, remaining)
            else:
                with gzip.open(physical, "rb") as stream:
                    copy_stream(stream, unpacked, remaining)
            child = child_location(location, 0, "decoded")
            with unpacked.open("rb") as stream:
                stage.file(stream, unpacked.stat().st_size, child, None, depth + 1)
        finally:
            unpacked.unlink(missing_ok=True)


def decode_zstd(source, destination, limit, timeout):
    try:
        proc = subprocess.Popen(
            ["zstd", "-d", "-q", "-c", "--memory=256MB", str(source)],
            stdout=subprocess.PIPE, stderr=subprocess.DEVNULL
        )
    except OSError as error:
        raise ScanError("zstd-unavailable") from error
    timer = threading.Timer(timeout, proc.kill)
    timer.start()
    try:
        copy_stream(proc.stdout, destination, limit)
        if proc.wait(timeout=timeout) != 0:
            raise ScanError("zstd-decompression-failed")
    finally:
        timer.cancel()
        timer.join()
        proc.stdout.close()
        if proc.poll() is None:
            proc.kill()
        proc.wait()
