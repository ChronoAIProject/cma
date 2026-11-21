"""Identify DRM drivers through their ABI, including Linux 7 faux VKMS devices."""
import ctypes
import errno
import json
import os
from pathlib import Path
import sys


class DrmVersion(ctypes.Structure):
    _fields_ = [
        ('major', ctypes.c_int), ('minor', ctypes.c_int), ('patch', ctypes.c_int),
        ('name_len', ctypes.c_size_t), ('name', ctypes.c_void_p),
        ('date_len', ctypes.c_size_t), ('date', ctypes.c_void_p),
        ('desc_len', ctypes.c_size_t), ('desc', ctypes.c_void_p),
    ]


def inventory():
    libc = ctypes.CDLL(None, use_errno=True)
    libc.ioctl.argtypes = [ctypes.c_int, ctypes.c_ulong, ctypes.c_void_p]
    libc.ioctl.restype = ctypes.c_int
    # DRM_IOCTL_VERSION = _IOWR('d', 0x00, struct drm_version).
    request = (3 << 30) | (ctypes.sizeof(DrmVersion) << 16) | (ord('d') << 8)
    devices = []
    for path in sorted(Path('/dev/dri').glob('card[0-9]*')):
        item = {'path': str(path)}
        try:
            fd = os.open(path, os.O_RDWR | os.O_CLOEXEC)
            try:
                name = ctypes.create_string_buffer(128)
                version = DrmVersion(name_len=len(name), name=ctypes.addressof(name))
                if libc.ioctl(fd, request, ctypes.byref(version)) != 0:
                    code = ctypes.get_errno()
                    raise OSError(code, os.strerror(code))
                if version.name_len >= len(name):
                    raise OSError(errno.EOVERFLOW, 'DRM driver name exceeds buffer')
                item['driver'] = name.raw[:version.name_len].decode('ascii')
            finally:
                os.close(fd)
        except OSError as error:
            item['error'] = str(error)
        devices.append(item)
    return devices


if __name__ == '__main__':
    devices = inventory()
    if sys.argv[1:] == ['--json']:
        print(json.dumps(devices))
    else:
        print(next((item['path'] for item in devices if item.get('driver') == 'vkms'), ''))
