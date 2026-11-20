"""Closed publication diagnostics; remote and subprocess text stays private."""
import json
import re
import subprocess
import urllib.error

from image_secret_gate import ScanRefusal
from image_scan_evidence import project as project_scan_evidence
from staging_cd_common import Refusal
from staging_cd_reservations import ReservationSuperseded
from staging_cd_runtime import SKIPPED, run_private

STAGES = frozenset({"validate", "resume", "snapshot", "reuse", "build", "registry_preflight",
                    "inspect", "scan", "runtime_smoke", "archive_check", "registry_login", "registry_copy",
                    "registry_verify", "receipt", "cache_restore", "cache_export"})
REASONS = frozenset({"registry_denied", "build_failed", "command_failed", "build_timeout", "command_timeout",
                     "scan_refused", "registry_read_failed", "validation_refused", "io_failed", "unexpected_error",
                     "reservation_owner_mismatch", "reservation_superseded"})


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
        self.scan_evidence = None

    def mark(self, stage):
        if stage not in STAGES:
            raise Refusal("Unknown publication diagnostic stage")
        self.stage = stage

    def classify(self, error):
        if isinstance(error, ReservationSuperseded):
            return "reservation_superseded"
        if isinstance(error, ImageCommandRefusal):
            if error.reason == "registry_denied":
                return "registry_denied"
            return "build_failed" if self.stage == "build" else "command_failed"
        if isinstance(error, subprocess.TimeoutExpired):
            return "build_timeout" if self.stage == "build" else "command_timeout"
        if isinstance(error, ScanRefusal):
            if isinstance(error.evidence, dict):
                # Re-project even exception fields: never trust arbitrary library text.
                self.scan_evidence = project_scan_evidence(error.evidence, error.outcome)
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


def run_publication(operation, *, on_superseded=None):
    diagnostics = PublicationDiagnostics()

    def guarded(masker):
        try:
            try:
                return operation(masker, diagnostics)
            except ReservationSuperseded:
                if on_superseded is None:
                    raise
                on_superseded()
                diagnostics.reason = "reservation_superseded"
                return SKIPPED
        except Exception as error:
            diagnostics.reason = diagnostics.classify(error)
            raise

    status = run_private(guarded, None, subject="publication", explain=False)
    if not status and diagnostics.reason == "reservation_superseded":
        print(f"SKIP publication stage={diagnostics.stage} reason=reservation_superseded")
    if status:
        # No exception text, paths, identifiers or scanner findings cross here.
        stage = diagnostics.stage if diagnostics.stage in STAGES else "validate"
        reason = diagnostics.reason if diagnostics.reason in REASONS else "unexpected_error"
        print(f"FAIL publication stage={stage} reason={reason}")
        if diagnostics.scan_evidence is not None:
            print("SCAN_EVIDENCE " + json.dumps(diagnostics.scan_evidence, sort_keys=True))
    return status
