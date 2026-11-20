"""Authorization and output boundaries for private deployment data."""
import contextlib
import io
import os
import sys
from pathlib import Path

SKIPPED = object()


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


def run_private(operation, summary_path):
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
        return private_result(operation, summary_path, output)


def private_result(operation, summary_path, output):
    def masker(value):
        mask_tree(value, lambda line: print(line, file=output, flush=True))

    result = "FAIL"
    streams = (sys.stdout, sys.stderr, sys.__stdout__, sys.__stderr__)
    # Library and subprocess diagnostics may contain private response fields.
    # Only GitHub masking commands, and ci_retry's fixed-field RETRY lines
    # (an operation name and numbers, written to the stdout descriptor it
    # duplicated at import), can bypass this output boundary.
    with open(os.devnull, "w", encoding="utf-8") as sink, \
            silence_descriptor(1, sink), silence_descriptor(2, sink), \
            contextlib.redirect_stdout(sink), contextlib.redirect_stderr(sink):
        try:
            value = operation(masker)
            if value is True:
                result = "PASS"
            elif value is SKIPPED:
                result = "SKIP"
        except Exception:  # noqa: BLE001 - public entry boundary must withhold private diagnostics and fail.
            result = "FAIL"
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
        result = "FAIL"
    print(result, file=output, flush=True)
    return int(result == "FAIL")
