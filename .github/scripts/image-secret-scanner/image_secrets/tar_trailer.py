"""Refuse tar readers' otherwise silent termination on unparsed trailing data."""

from .common import ScanError


def verify_trailer(archive, byte_limit):
    # TarFile.offset is the end of the last parsed member, before the first
    # end marker (or invalid header). Include that marker in validation.
    stream = archive.fileobj
    stream.seek(archive.offset)
    size = 0
    while True:
        block = stream.read(65536)
        if not block:
            break
        size += len(block)
        if archive.offset + size > byte_limit:
            raise ScanError("tar-trailer-byte-limit")
        if any(block):
            raise ScanError("tar-nonzero-trailer")
    if size < 1024 or size % 512:
        raise ScanError("tar-invalid-trailer")
