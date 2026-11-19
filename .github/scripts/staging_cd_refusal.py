"""Read only a fixed, request-bound reason after a controller returns 30."""
import os
import stat

from staging_cd_receipts import sha256

REASONS = frozenset({
    "permit", "authentication", "authorization", "clock", "inventory", "compatibility",
    "image-version", "image-identity", "baseline", "readiness", "transport", "acceptance",
    "schema", "fence", "interrupted", "preflight",
})


def read(path, request):
    # Open without following links or waiting on an unexpected FIFO.
    try:
        fd = os.open(path, os.O_RDONLY | os.O_NONBLOCK | os.O_NOFOLLOW)
        try:
            info = os.fstat(fd)
            if not stat.S_ISREG(info.st_mode) or info.st_mode & 0o077 or info.st_size > 128:
                return None
            data = os.read(fd, 129)
        finally:
            os.close(fd)
    except OSError:
        return None
    digest = sha256(request)
    for reason in REASONS:
        if data == f"{digest} {reason}\n".encode("ascii"):
            return reason
    return None
