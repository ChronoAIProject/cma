"""Allow only fixed, request-bound staging failure diagnostics."""
import os
import stat

from staging_cd_receipts import sha256

REASONS = frozenset({
    "permit", "authentication", "authorization", "clock", "inventory", "compatibility",
    "image-version", "image-identity", "baseline", "readiness", "transport", "acceptance",
    "schema", "fence", "configuration", "publication", "internal-error", "interrupted",
    "preflight",
    # chrono-cd stored a generation but never applied it; an operator must apply it.
    "apply-abandoned",
    # Production: Agent Manager runs more replicas than its release budget covers.
    "am-replicas",
})


VERIFICATION_REASONS = frozenset({
    "write-timeout", "write-response", "read-timeout", "read-response", "status-reconcile",
    "status-counts", "status-inventory", "status-pods", "pod-restarts",
    "pod-restarts-membership", "pod-restarts-startup-gate", "pod-restarts-panic", "rollout-ownership",
    "rollout-timeout", "rollout-failed", "image-digest", "ownership", "recovery-ownership",
    "recovery-revision", "phase-budget", "image-identity", "served-identity", "status-shape",
    "pod-evidence", "cohort", "serving", "acceptance", "startup-observation", "startup-schema",
    "namespace-fence", "dependency-changed", "dependency-unverified", "interrupted", "result", "unreported",
    # The controller names every other failure with its coarse refusal cause.
    *(REASONS - {"preflight"}),
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


# A packet bound to this request whose token this repository does not know:
# the controller's vocabulary is ahead of this catalogue. The token itself is
# never returned, so it cannot carry controller text into the public log.
UNRECOGNIZED = "unrecognized"


def bound_tokens(path, request, count):
    """Return a well-formed packet's tokens if it is bound to request, else None."""
    data = packet(path)
    if data is None or not data.endswith(b"\n") or b"\n" in data[:-1]:
        return None
    fields = data[:-1].split(b" ")
    if len(fields) != count + 1 or fields[0] != sha256(request).encode("ascii"):
        return None
    return fields[1:]


def known(field, vocabulary):
    try:
        token = field.decode("ascii")
    except UnicodeDecodeError:
        return UNRECOGNIZED
    return token if token in vocabulary else UNRECOGNIZED


def read(path, request):
    """Return the refusal reason, "unrecognized", or None when no bound packet exists."""
    fields = bound_tokens(path, request, 1)
    return None if fields is None else known(fields[0], REASONS)


def read_verification(path, request):
    """Return (verification, recovery) tokens, each known or "unrecognized"."""
    fields = bound_tokens(path, request, 2)
    return None if fields is None else tuple(known(field, VERIFICATION_REASONS) for field in fields)
