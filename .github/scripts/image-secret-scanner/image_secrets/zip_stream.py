"""Validate full compressed ZIP streams independently of declared output sizes."""

import tempfile
import zlib
from contextlib import contextmanager

from .common import MAX_NESTED_MEMBER_BYTES, ScanError
from .zip_framing import payload_extent, require


def decode(stream, output, member, compressed, limit):
    decoder = zlib.decompressobj(-15) if member.compress_type == 8 else None
    total = crc = 0
    pending = b""
    while compressed or pending or (decoder is not None and not decoder.eof):
        if not pending and compressed:
            pending = stream.read(min(compressed, 65536))
            require(bool(pending))
            compressed -= len(pending)
        if decoder is None:
            block, pending = pending, b""
        else:
            block = decoder.decompress(pending, min(65536, limit - total + 1))
            pending = decoder.unconsumed_tail
            require(not decoder.unused_data)
            if decoder.eof:
                require(not pending and compressed == 0)
            elif not block and not pending and compressed == 0:
                raise ScanError("nested-zip-stream-truncated")
        total += len(block)
        if total > limit:
            raise ScanError("nested-zip-expanded-size-limit")
        if total > member.file_size:
            raise ScanError("nested-zip-size-mismatch")
        crc = zlib.crc32(block, crc)
        output.write(block)
    require(total == member.file_size and crc == member.CRC)


@contextmanager
def verified_member(path, member, end, remaining):
    if member.file_size > MAX_NESTED_MEMBER_BYTES:
        raise ScanError("nested-member-size-limit")
    if member.file_size > remaining:
        raise ScanError("byte-budget-exceeded")
    limit = min(MAX_NESTED_MEMBER_BYTES, remaining)
    # Private, synthetic, automatically closed scratch: no archive path is
    # opened, no link is followed, and unverified output cannot yield CLEAN.
    with path.open("rb") as source, tempfile.TemporaryFile(dir=path.parent) as output:
        compressed = payload_extent(source, member, end)
        try:
            decode(source, output, member, compressed, limit)
        except zlib.error as error:
            raise ScanError("nested-zip-deflate-invalid") from error
        output.seek(0)
        yield output
