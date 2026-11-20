"""Narrow bindings to libarchive's maintained contained-ar reader."""

import ctypes
import ctypes.util

from .common import ScanError


class ArReader:
    def __init__(self, path):
        name = ctypes.util.find_library("archive")
        if not name:
            raise ScanError("ar-decoder-unavailable")
        self.lib = ctypes.CDLL(name)
        pointer, integer = ctypes.c_void_p, ctypes.c_int

        def bind(name, arguments, result):
            function = getattr(self.lib, name)
            function.argtypes, function.restype = arguments, result
            return function

        self.new = bind("archive_read_new", [], pointer)
        self.free = bind("archive_read_free", [pointer], integer)
        self.support = bind("archive_read_support_format_ar", [pointer], integer)
        self.filter_none = bind("archive_read_support_filter_none", [pointer], integer)
        self.open = bind("archive_read_open_filename", [pointer, ctypes.c_char_p, ctypes.c_size_t], integer)
        self.next = bind("archive_read_next_header", [pointer, ctypes.POINTER(pointer)], integer)
        self.pathname = bind("archive_entry_pathname", [pointer], ctypes.c_char_p)
        self.size = bind("archive_entry_size", [pointer], ctypes.c_int64)
        self.filetype = bind("archive_entry_filetype", [pointer], ctypes.c_uint)
        self.data = bind("archive_read_data", [pointer, pointer, ctypes.c_size_t], ctypes.c_ssize_t)
        self.handle = self.new()
        if not self.handle:
            raise ScanError("ar-decoder-unavailable")
        try:
            self.require(self.support(self.handle) == 0)
            self.require(self.filter_none(self.handle) == 0)
            self.require(self.open(self.handle, bytes(path), 65536) == 0)
        except BaseException:
            self.close()
            raise

    @staticmethod
    def require(condition):
        if not condition:
            raise ScanError("ar-decoder-refused")

    def header(self):
        entry = ctypes.c_void_p()
        status = self.next(self.handle, ctypes.byref(entry))
        if status == 1:
            return None
        # Warnings and retries are uncertain decoding, never accepted entries.
        # GNU ar commonly stores only permission bits (0644), so libarchive
        # reports no file-type bits. Physical framing already forbids links.
        self.require(status == 0 and entry.value and self.filetype(entry) in (0, 0o100000))
        name = self.pathname(entry)
        self.require(name is not None and len(name) <= 16384)
        size = self.size(entry)
        self.require(size >= 0)
        return name.decode("utf-8", "surrogateescape"), size

    def chunks(self, size):
        buffer = ctypes.create_string_buffer(65536)
        remaining = size
        while remaining:
            count = self.data(self.handle, buffer, min(remaining, len(buffer)))
            self.require(0 < count <= remaining)
            remaining -= count
            yield buffer.raw[:count]
        self.require(self.data(self.handle, buffer, 1) == 0)

    def close(self):
        if self.handle:
            handle, self.handle = self.handle, None
            self.require(self.free(handle) == 0)
