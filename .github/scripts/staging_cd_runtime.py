"""Authorization and output boundaries for private deployment data."""
import contextlib
import io
import os
import subprocess
import sys
import urllib.error
from pathlib import Path

from ci_outcome import error_site, field, report
from staging_cd_common import Refusal

SKIPPED = object()
# Failure kinds, explained by the "step" catalogue family.
KINDS = frozenset({"refused", "timeout", "network", "http-server", "http-denied", "http-client", "os-error",
                   "error", "declined"})


def mask_tree(value, emit):
    values = set()

    def collect(item):
        if isinstance(item, str) and item:
            values.add(item)
        elif isinstance(item, dict):
            for key, child in item.items():
                collect(key)
                collect(child)
        elif isinstance(item, list):
            for child in item:
                collect(child)

    collect(value)
    for item in sorted(values, key=lambda item: (-len(item), item)):
        escaped = item.replace("%", "%25").replace("\r", "%0D").replace("\n", "%0A")
        emit(f"::add-mask::{escaped}")


@contextlib.contextmanager
def silence_descriptor(descriptor, sink):
    saved = os.dup(descriptor)
    try:
        os.dup2(sink.fileno(), descriptor)
        yield
    finally:
        os.dup2(saved, descriptor)
        os.close(saved)


def failure_kind(error):
    """Classify an exception by type and status only; its text may carry private data.

    Only explicit signals mean "retry may help": a timeout, a failed connection
    or an HTTP 5xx or 429. Other OS errors, such as a missing file, and other
    HTTP 4xx responses are not presumed transient.
    """
    if isinstance(error, Refusal):
        return "refused"
    if isinstance(error, (subprocess.TimeoutExpired, TimeoutError)):
        return "timeout"
    if isinstance(error, urllib.error.HTTPError):
        if error.code in (401, 403):
            return "http-denied"
        return "http-server" if error.code == 429 or 500 <= error.code < 600 else "http-client"
    if isinstance(error, (urllib.error.URLError, ConnectionError)):
        return "network"
    if isinstance(error, OSError):
        return "os-error"
    return "error"


def run_private(operation, summary_path, *, subject, explain=True):
    """Run operation with private output and print "PASS|SKIP|FAIL <subject>".

    With explain, a failure line also names its kind and the public source
    line that failed, with an annotation saying what to do. Callers with a
    more specific vocabulary pass explain=False and report it themselves.
    """
    # Python stream redirection alone does not cover native libraries or child
    # processes. Keep a separate public descriptor solely for masking commands.
    with contextlib.ExitStack() as stack:
        output = sys.stdout
        output.flush()
        sys.stderr.flush()
        try:
            descriptor = output.fileno()
        except (AttributeError, io.UnsupportedOperation):
            pass  # In-memory callers have no public descriptor to preserve.
        else:
            output = stack.enter_context(os.fdopen(os.dup(descriptor), "w", encoding="utf-8"))
        return private_result(operation, summary_path, output, subject=subject, explain=explain)


def private_result(operation, summary_path, output, *, subject, explain=False):
    def masker(value):
        mask_tree(value, lambda line: print(line, file=output, flush=True))

    result, kind, site = "FAIL", "declined", None
    streams = (sys.stdout, sys.stderr, sys.__stdout__, sys.__stderr__)
    # Library and subprocess diagnostics may contain private response fields.
    # Only GitHub masking commands, ci_retry's fixed-field RETRY lines (an
    # operation name and numbers, written to the stdout descriptor it
    # duplicated at import), and allocation's PROGRESS and RETRY allocation
    # lines (closed categories, a fixed code and numbers, written to the
    # descriptor allocation_progress duplicated before entering this
    # boundary) can bypass it.
    with open(os.devnull, "w", encoding="utf-8") as sink, \
            silence_descriptor(1, sink), silence_descriptor(2, sink), \
            contextlib.redirect_stdout(sink), contextlib.redirect_stderr(sink):
        try:
            value = operation(masker)
            if value is True:
                result = "PASS"
            elif value is SKIPPED:
                result = "SKIP"
        except Exception as error:  # noqa: BLE001 - public entry boundary must withhold private diagnostics and fail.
            result, kind, site = "FAIL", failure_kind(error), error_site(error)
        finally:
            # Drain references to the original streams before restoring their
            # descriptors, so deferred writes cannot escape after the operation.
            for stream in streams:
                if stream is not None:
                    stream.flush()
    try:
        if summary_path is not None:
            Path(summary_path).write_text(result + "\n", encoding="utf-8")
    except OSError:
        result, kind, site = "FAIL", "os-error", None
    if result == "FAIL" and explain:
        report("step", kind, subject=subject, site=site, stream=output)
    else:
        print(result + " " + field("subject", subject), file=output, flush=True)
    return int(result == "FAIL")
