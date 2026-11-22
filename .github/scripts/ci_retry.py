"""Bounded retries for transient network failures shared by CI and CD scripts.

Reads repeat with exponential backoff and full jitter inside an absolute
deadline. Writes run exactly once; an uncertain write is settled by reading
the target back, never by repeating it blindly.

This module is a leaf. It uses only the standard library and imports no
repository module, so every script can share one policy and one log format.
"""
import email.utils
import errno
import http.client
import os
import random
import re
import socket
import ssl
import subprocess
import sys
import time
import urllib.error

TRANSIENT = "transient"
PERMANENT = "permanent"
COMMITTED = "committed"
ABSENT = "absent"
CONFLICT = "conflict"

# Operation names are fixed literals chosen by the caller, so a retry line
# never carries a path, URL, identifier or remote diagnostic.
NAME = re.compile(r"[a-z][a-z0-9-]{0,62}")
# Diagnostics can be long; the failing request is reported at the end.
SCAN_LIMIT = 64 * 1024


class TransientError(Exception):
    """Marks one failed attempt as safe to repeat; raise or chain it with `from`.

    `after` is the wait in seconds the server asked for, when it said so.
    """

    # A subclass that also derives from another exception may never reach
    # __init__; it then asked for no wait rather than lacking the attribute.
    after = None

    def __init__(self, *args, after=None):
        super().__init__(*args)
        self.after = after


class PermanentError(Exception):
    """Ends classification: never repeat, whatever the cause chain holds."""


class WriteUncertain(PermanentError):
    """A write failed in transport and its target could not be read back."""


class WriteConflict(PermanentError):
    """A write's target holds a value this write did not produce."""


def header(headers, name):
    """A header value from raw `Name: value` lines or a mapping, matched case-insensitively."""
    if isinstance(headers, str):
        rows = (line.partition(":")[::2] for line in headers.splitlines())
    elif hasattr(headers, "items"):
        rows = headers.items()
    else:
        return None
    for key, value in rows:
        if str(key).strip().lower() == name:
            return str(value).strip()
    return None


def throttled(headers):
    """Only explicit signals: a Retry-After header or an exhausted rate-limit quota."""
    return header(headers, "retry-after") is not None or header(headers, "x-ratelimit-remaining") == "0"


def http_status(code, headers=None):
    """Timeouts, throttling and server errors are transient; other statuses are not.

    A 403 is throttling only when its headers say so; its message text never decides.
    """
    if code in (408, 425, 429) or 500 <= code <= 599:
        return TRANSIENT
    return TRANSIENT if code == 403 and throttled(headers) else PERMANENT


STATUS = re.compile(r"(?:\bHTTP(?:/[0-9.]+)?|\b(?<!exit )status(?:[ _]?code)?|returned error)\s*[:=]?\s*\(?([1-5][0-9]{2})\b",
                    re.IGNORECASE)
TRANSPORT = re.compile("|".join((
    r"connection (?:reset|refused|aborted|closed)", r"broken pipe", r"timed? ?out", r"deadline exceeded",
    r"temporary failure", r"no such host", r"could not resolve", r"server misbehaving",
    r"network is unreachable", r"no route to host", r"tls handshake", r"bad record mac", r"ssl_error_syscall",
    r"gnutls", r"http2:", r"goaway", r"stream error", r"error connecting to", r"hung up unexpectedly",
    r"use of closed network connection", r"rpc failed", r"bad gateway", r"service unavailable", r"gateway time-?out",
    r"too ?many ?requests",
)), re.IGNORECASE)
# A truncated stream is transport only when the message names the remote end;
# the same words from reading a local archive mean the file itself is damaged.
EOF = re.compile(r"\beof\b", re.IGNORECASE)
REMOTE = re.compile(r"https?://|\btcp\b|\btls:", re.IGNORECASE)


def cli_message(text):
    """Classify a command-line client's diagnostic text (gh, git, skopeo, curl)."""
    if isinstance(text, bytes):
        text = text.decode("utf-8", errors="replace")
    if not isinstance(text, str):
        return PERMANENT
    text = text[-SCAN_LIMIT:]
    codes = [int(code) for code in STATUS.findall(text)]
    if codes:
        # An HTTP response arrived; its status decides. Text carries no headers,
        # so a 403 here is never read as throttling.
        return TRANSIENT if any(http_status(code) == TRANSIENT for code in codes) else PERMANENT
    if TRANSPORT.search(text) or (EOF.search(text) and REMOTE.search(text)):
        return TRANSIENT
    return PERMANENT


def cli_cause(text):
    """A cause for `raise Refusal(...) from cli_cause(stderr)`; None when permanent."""
    return TransientError() if cli_message(text) == TRANSIENT else None


# Header numbers are plain ASCII seconds; int() would accept other scripts' digits.
DIGITS = re.compile(r"[0-9]{1,10}")
# An opted-in read waits at most this long for one throttled response.
RETRY_AFTER_CAP = 60.0


def retry_after(headers, now=None):
    """Seconds a throttled response asked to wait: Retry-After, or an exhausted quota's reset."""
    value = header(headers, "retry-after")
    if value is not None:
        if DIGITS.fullmatch(value):
            return min(int(value), 86400)
        try:
            when = email.utils.parsedate_to_datetime(value).timestamp()
        except (TypeError, ValueError):
            return None
        return max(0.0, min(when - (time.time() if now is None else now), 86400))
    reset = header(headers, "x-ratelimit-reset")
    if header(headers, "x-ratelimit-remaining") == "0" and reset is not None and DIGITS.fullmatch(reset):
        return max(0.0, min(int(reset) - (time.time() if now is None else now), 86400))
    return None


def status_cause(code, headers=None):
    """A cause for `raise Refusal(...) from status_cause(code, headers)`; None when permanent."""
    return TransientError(after=retry_after(headers)) if http_status(code, headers) == TRANSIENT else None


def requested_wait(error):
    """The longest wait a transient cause in this chain asked for, or None."""
    waits = []
    for _ in range(16):
        if error is None:
            break
        if isinstance(error, TransientError) and error.after is not None:
            waits.append(error.after)
        error = error.__cause__
    return max(waits, default=None)


TRANSIENT_ERRNOS = frozenset(getattr(errno, name) for name in (
    "ECONNRESET", "ECONNREFUSED", "ECONNABORTED", "ETIMEDOUT", "EPIPE", "ENETUNREACH",
    "ENETDOWN", "ENETRESET", "EHOSTUNREACH", "EHOSTDOWN") if hasattr(errno, name))
TRANSIENT_TYPES = (subprocess.TimeoutExpired, TimeoutError, ConnectionError, socket.gaierror,
                   http.client.IncompleteRead, http.client.BadStatusLine,
                   ssl.SSLEOFError, ssl.SSLZeroReturnError, ssl.SSLSyscallError)


def classify(error):
    """Follow explicit causes (`raise ... from`) until one names the failure."""
    for _ in range(16):
        if error is None:
            break
        if isinstance(error, PermanentError):
            return PERMANENT
        if isinstance(error, TransientError):
            return TRANSIENT
        if isinstance(error, urllib.error.HTTPError):
            return http_status(error.code, error.headers)
        if isinstance(error, urllib.error.URLError):
            # A URLError wraps the transport exception; a text reason is a usage error.
            if not isinstance(error.reason, BaseException):
                return PERMANENT
            error = error.reason
            continue
        if isinstance(error, TRANSIENT_TYPES):
            return TRANSIENT
        if isinstance(error, OSError) and error.errno in TRANSIENT_ERRNOS:
            return TRANSIENT
        error = error.__cause__
    return PERMANENT


def _public_descriptor():
    # Private boundaries redirect descriptors 1 and 2 to /dev/null. Retry lines
    # hold only a fixed name and numbers, so keep the public descriptor that
    # existed when this module was imported, before any such redirection.
    try:
        return os.dup(sys.stdout.fileno())
    except (AttributeError, OSError, ValueError):
        return None


_PUBLIC = _public_descriptor()


def emit(line):
    if _PUBLIC is None:
        print(line, flush=True)
        return
    try:
        sys.stdout.flush()  # keep earlier buffered output ahead of this line
    except (OSError, ValueError):
        pass
    # One short write is atomic on a pipe, so lines from parallel reads never interleave.
    os.write(_PUBLIC, (line + "\n").encode("utf-8"))


def attempt_timeout(cap, deadline, clock=time.monotonic):
    """One attempt's timeout: `cap`, shortened to what remains before an absolute `deadline`.

    None means the deadline has passed; the caller refuses instead of starting.
    """
    if deadline is None:
        return cap
    left = deadline - clock()
    return min(cap, left) if left > 0 else None


def bounded(policy, deadline):
    """A copy of a retry policy whose repeats also end at an absolute `deadline`."""
    policy = dict(policy or {})
    if deadline is not None:
        current = policy.get("deadline")
        policy["deadline"] = deadline if current is None else min(current, deadline)
    return policy


def backoff(attempt, base, cap, rand):
    """Full jitter: a uniform wait up to the capped exponential bound for this attempt."""
    jitter = rand()
    if not 0 <= jitter <= 1:
        raise ValueError("Retry jitter source is out of range")
    return jitter * min(cap, base * 2 ** (attempt - 1))


def call(op, *, name, classify, attempts=3, base=1.0, cap=20.0, deadline=None,
         clock=time.monotonic, sleep=time.sleep, rand=random.random, log=None, honour_retry_after=False):
    """Run a read; repeat transient failures up to `attempts` times inside `deadline`.

    `deadline` is an absolute `clock()` value. A wait that would reach it is
    never started; the last error is raised instead. Permanent failures and the
    final attempt's failure propagate unchanged. With `honour_retry_after`, a
    wait is at least what a throttled response asked for, up to RETRY_AFTER_CAP.
    """
    if not NAME.fullmatch(name if isinstance(name, str) else ""):
        raise ValueError("Retry operation name must be a fixed lowercase literal")
    if type(attempts) is not int or attempts < 1 or base < 0 or cap < 0:
        raise ValueError("Retry bounds are invalid")
    attempt = 1
    while True:
        try:
            return op()
        except Exception as error:  # noqa: BLE001 - classification decides; others re-raise unchanged.
            if attempt >= attempts or classify(error) != TRANSIENT:
                raise
            wait = backoff(attempt, base, cap, rand)
            if honour_retry_after:
                wait = max(wait, min(requested_wait(error) or 0, RETRY_AFTER_CAP))
            if deadline is not None and clock() + wait >= deadline:
                raise
            attempt += 1
            (log or emit)(f"RETRY op={name} attempt={attempt}/{attempts} wait={wait:.1f}s")
            sleep(wait)


def call_write(op, readback, *, name, classify, readback_attempts=3, **policy):
    """Run a write exactly once and settle a transient (uncertain) failure by readback.

    `readback()` returns COMMITTED, ABSENT or CONFLICT and is retried under
    `policy`. COMMITTED returns None; CONFLICT raises WriteConflict; ABSENT
    re-raises the write's own transient failure, so a caller may repeat the
    write only after this proof that the target was untouched. An unreadable
    target raises WriteUncertain, which no policy repeats.
    """
    try:
        return op()
    except Exception as error:  # noqa: BLE001 - classification decides; others re-raise unchanged.
        if classify(error) != TRANSIENT:
            raise
        failure = error
    try:
        state = call(readback, name=name + "-readback", classify=classify, attempts=readback_attempts, **policy)
    except Exception as error:  # noqa: BLE001 - any readback failure leaves the write unresolved.
        raise WriteUncertain("Write outcome could not be read back") from error
    if state == COMMITTED:
        return None
    if state == ABSENT:
        raise failure
    if state == CONFLICT:
        raise WriteConflict("Write target holds a different value") from None
    raise WriteUncertain("Write readback is invalid")
