"""Closed publication diagnostics; remote and subprocess text stays private."""
import re
import subprocess
import urllib.error

from image_secret_gate import ScanRefusal
from staging_cd_common import Refusal
from staging_cd_runtime import run_private

STAGES = frozenset({"validate", "resume", "snapshot", "reuse", "build", "registry_preflight",
                    "inspect", "scan", "archive_check", "registry_login", "registry_copy",
                    "registry_verify", "receipt"})


class ImageCommandRefusal(Refusal):
    def __init__(self, args, stderr):
        # Match only diagnostic tokens; never interpolate remote text or argv.
        registry = len(args) > 1 and args[0] == "skopeo" and args[1] in {"login", "copy", "inspect"}
        diagnostic = stderr.decode("utf-8", errors="replace") if isinstance(stderr, bytes) else ""
        denied = re.search(r"\b(?:UNAUTHORIZED|DENIED|permission_denied|write_package)\b"
                           r"|(?:status(?: code)?|response status)\s*:?\s*(?:401|403)\b",
                           diagnostic, re.IGNORECASE)
        self.reason = "registry_denied" if registry and denied else "command_failed"
        super().__init__("Image operation failed; captured output is private")


class PublicationDiagnostics:
    def __init__(self):
        self.stage = "validate"
        self.reason = "unexpected_error"

    def mark(self, stage):
        if stage not in STAGES:
            raise Refusal("Unknown publication diagnostic stage")
        self.stage = stage

    def classify(self, error):
        if isinstance(error, ImageCommandRefusal):
            if error.reason == "registry_denied":
                return "registry_denied"
            return "build_failed" if self.stage == "build" else "command_failed"
        if isinstance(error, subprocess.TimeoutExpired):
            return "build_timeout" if self.stage == "build" else "command_timeout"
        if isinstance(error, ScanRefusal):
            return "scan_refused"
        # The read adapter chains HTTPError without exposing its response body.
        cause = error.__cause__
        if self.stage.startswith("registry_") and isinstance(cause, urllib.error.HTTPError):
            return "registry_denied" if cause.code in (401, 403) else "registry_read_failed"
        if isinstance(error, Refusal):
            if str(error) == "Reservation owner differs":
                return "reservation_owner_mismatch"
            return "validation_refused"
        if isinstance(error, OSError):
            return "io_failed"
        return "unexpected_error"


def run_publication(operation):
    diagnostics = PublicationDiagnostics()

    def guarded(masker):
        try:
            return operation(masker, diagnostics)
        except Exception as error:
            diagnostics.reason = diagnostics.classify(error)
            raise

    status = run_private(guarded, None)
    if status:
        # No exception text, paths, identifiers or scanner findings cross here.
        stage = diagnostics.stage if diagnostics.stage in STAGES else "validate"
        reasons = {"registry_denied", "build_failed", "command_failed", "build_timeout", "command_timeout",
                   "scan_refused", "registry_read_failed", "validation_refused", "io_failed", "unexpected_error",
                   "reservation_owner_mismatch"}
        reason = diagnostics.reason if diagnostics.reason in reasons else "unexpected_error"
        print(f"FAIL publication stage={stage} reason={reason}")
    return status
