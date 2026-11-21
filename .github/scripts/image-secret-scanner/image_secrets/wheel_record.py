"""Verify every member of a repacked wheel against its complete RECORD."""

import base64
import csv
import hashlib
import io
import zipfile

from .common import archive_path, digest_bytes
from .container_limits import zip_directory
from .zip_framing import leading_prefix, member_ends
from .zip_stream import verified_member

MAX_RECORD_BYTES = 1024 * 1024


def verify(path, budget):
    directory = zip_directory(path, budget.max_entries - budget.entries)
    expected = {}
    record = None
    with zipfile.ZipFile(path) as archive:
        entries = archive.infolist()
        ends = member_ends(entries, directory)
        if leading_prefix(path, min(ends, default=directory)):
            return None
        for entry in entries:
            budget.entry()
            name = archive_path(entry.filename)
            if name != entry.filename or name in expected or entry.flag_bits & 1:
                return None
            with verified_member(path, entry, ends[entry.header_offset], budget.max_bytes - budget.bytes) as (body, regular):
                if not regular:
                    return None
                budget.reserve(entry.file_size)
                if name.endswith(".dist-info/RECORD"):
                    if record is not None or entry.file_size > MAX_RECORD_BYTES:
                        return None
                    record = body.read(MAX_RECORD_BYTES + 1)
                    expected[name] = ("", "")
                else:
                    checksum = hashlib.sha256()
                    for block in iter(lambda: body.read(65536), b""):
                        checksum.update(block)
                    encoded = base64.urlsafe_b64encode(checksum.digest()).rstrip(b"=").decode("ascii")
                    expected[name] = ("sha256=" + encoded, str(entry.file_size))
    if record is None:
        return None
    try:
        rows = csv.reader(io.StringIO(record.decode("utf-8"), newline=""), strict=True)
        for row in rows:
            if len(row) != 3 or expected.pop(row[0], None) != (row[1], row[2]):
                return None
    except (UnicodeDecodeError, csv.Error):
        return None
    return None if expected else digest_bytes(record)
