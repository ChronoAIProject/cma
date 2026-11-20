"""Bound XZ dictionary allocation and output, including concatenated streams."""

import lzma

from .common import MAX_NESTED_MEMBER_BYTES, ScanError


def decode_xz(source, destination, limit):
    with source.open("rb") as stream, destination.open("xb") as output:
        pending = stream.read(65536)
        total = 0
        while pending:
            decoder = lzma.LZMADecompressor(format=lzma.FORMAT_XZ, memlimit=MAX_NESTED_MEMBER_BYTES)
            while not decoder.eof:
                block = decoder.decompress(pending, max_length=min(65536, limit - total + 1))
                pending = b""
                total += len(block)
                if total > limit:
                    raise ScanError("expanded-layer-budget-exceeded")
                output.write(block)
                if decoder.needs_input and not decoder.eof:
                    pending = stream.read(65536)
                    if not pending:
                        raise ScanError("nested-xz-truncated")
            pending = decoder.unused_data or stream.read(65536)
            # XZ stream padding is a multiple of four zero bytes. Anything else
            # must be another validated XZ stream, never silently ignored data.
            padding = 0
            while pending.startswith(b"\x00"):
                count = len(pending) - len(pending.lstrip(b"\x00"))
                padding += count
                pending = pending[count:] or stream.read(65536)
            if padding % 4:
                raise ScanError("nested-xz-invalid-padding")
