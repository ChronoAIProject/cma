"""Allow only fixed, request-bound staging failure diagnostics."""
import os
import stat

from staging_cd_receipts import sha256

REASONS = frozenset({
    "permit", "authentication", "authorization", "clock", "inventory", "compatibility",
    "image-version", "image-identity", "baseline", "readiness", "transport", "acceptance",
    "schema", "fence", "interrupted", "preflight",
})


VERIFICATION_REASONS = frozenset({
    "write-timeout", "write-response", "read-timeout", "read-response", "status-reconcile",
    "status-counts", "status-inventory", "status-pods", "pod-restarts", "rollout-ownership",
    "rollout-timeout", "rollout-failed", "image-digest", "ownership", "recovery-ownership",
    "recovery-revision", "phase-budget", "image-identity", "served-identity", "status-shape",
    "pod-evidence", "cohort", "serving", "acceptance", "startup-observation", "startup-schema",
    "namespace-fence", "interrupted", "result", "unreported",
})


def packet(path):
    # Open without following links or waiting on an unexpected FIFO.
    try:
        fd = os.open(path, os.O_RDONLY | os.O_NONBLOCK | os.O_NOFOLLOW)
        try:
            info = os.fstat(fd)
            if not stat.S_ISREG(info.st_mode) or info.st_mode & 0o077 or info.st_size > 128:
                return None
            return os.read(fd, 129)
        finally:
            os.close(fd)
    except OSError:
        return None


def read(path, request):
    data = packet(path)
    digest = sha256(request)
    for reason in REASONS:
        if data == f"{digest} {reason}\n".encode("ascii"):
            return reason
    return None

def read_verification(path, request):
    data = packet(path)
    if data is None or not data.endswith(b"\n"):
        return None
    fields = data[:-1].split(b" ")
    if len(fields) != 3 or fields[0] != sha256(request).encode("ascii"):
        return None
    try:
        reasons = tuple(field.decode("ascii") for field in fields[1:])
    except UnicodeDecodeError:
        return None
    return reasons if all(reason in VERIFICATION_REASONS for reason in reasons) else None
