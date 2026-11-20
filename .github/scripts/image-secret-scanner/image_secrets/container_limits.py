"""Bound parser metadata before standard-library decoders allocate it."""

import struct

from .common import MAX_NESTED_MEMBER_BYTES, ScanError

MAX_ZIP_DIRECTORY_BYTES = 16 * 1024 * 1024


def require(condition):
    if not condition:
        raise ScanError("nested-zip-directory-invalid")


class BoundedReader:
    def __init__(self, stream):
        self.stream = stream

    def read(self, size=-1):
        if size < 0 or size > MAX_NESTED_MEMBER_BYTES:
            raise ScanError("nested-metadata-size-limit")
        return self.stream.read(size)

    def __getattr__(self, name):
        return getattr(self.stream, name)


def zip_directory(path, entry_limit):
    # ZipFile eagerly reads its directory and builds every ZipInfo. Validate
    # physical record counts and bytes first; do not trust an EOCD count alone.
    size = path.stat().st_size
    with path.open("rb") as stream:
        start = max(0, size - 22 - 65535)
        stream.seek(start)
        tail = stream.read()
        offset = tail.rfind(b"PK\x05\x06")
        require(offset >= 0 and offset + 22 <= len(tail))
        _, disk, cd_disk, disk_count, count, cd_size, cd_offset, comment = struct.unpack(
            "<4s4H2LH", tail[offset:offset + 22])
        require(disk == cd_disk == 0 and disk_count == count and offset + 22 + comment <= len(tail))
        end = start + offset
        directory_end = end
        if end >= 20:
            stream.seek(end - 20)
            locator = stream.read(20)
            if locator.startswith(b"PK\x06\x07"):
                _, disk64, record_offset, disks = struct.unpack("<4sLQL", locator)
                require(disk64 == 0 and disks == 1 and end >= 76)
                stream.seek(end - 76)
                record = stream.read(56)
                fields = struct.unpack("<4sQ2H2L4Q", record)
                require(fields[0] == b"PK\x06\x06" and fields[1] == 44
                        and fields[4] == fields[5] == 0 and fields[6] == fields[7])
                # Extended ZIP64 end-record data is outside this bounded subset.
                require(count in (0xffff, fields[7]) and cd_size in (0xffffffff, fields[8])
                        and cd_offset in (0xffffffff, fields[9]))
                count, cd_size, cd_offset = fields[7:10]
                directory_end = end - 76
                require(0 <= record_offset <= directory_end)
        if count > entry_limit:
            raise ScanError("entry-budget-exceeded")
        if cd_size > MAX_ZIP_DIRECTORY_BYTES:
            raise ScanError("nested-zip-directory-size-limit")
        directory_start = directory_end - cd_size
        require(0 <= cd_offset <= directory_start <= directory_end)
        stream.seek(directory_start)
        observed = 0
        while stream.tell() < directory_end:
            observed += 1
            if observed > entry_limit:
                raise ScanError("entry-budget-exceeded")
            header = stream.read(46)
            require(len(header) == 46 and header.startswith(b"PK\x01\x02"))
            name, extra, comment, disk = struct.unpack("<4H", header[28:36])
            require(disk == 0)
            next_record = stream.tell() + name + extra + comment
            require(next_record <= directory_end)
            stream.seek(next_record)
        require(stream.tell() == directory_end and observed == count)
        return directory_start
