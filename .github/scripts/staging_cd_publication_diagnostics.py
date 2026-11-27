"""Closed publication diagnostics; remote and subprocess text stays private."""
import json
import re
import subprocess
import urllib.error

from ci_outcome import error_site, report
from image_secret_gate import ScanRefusal
from image_scan_evidence import project as project_scan_evidence
from staging_cd_common import Refusal
from staging_cd_reservations import ReservationSuperseded
from staging_cd_runtime import SKIPPED, run_private

STAGES = frozenset({"validate", "resume", "snapshot", "reuse", "build", "registry_preflight",
                    "inspect", "scan", "runtime_smoke", "archive_check", "registry_login", "registry_copy",
                    "registry_verify", "receipt", "cache_restore", "cache_export"})
# Fixed refusal messages with their own reason; any other refusal is validation_refused.
REFUSAL_REASONS = {
    "Reservation owner differs": "reservation_owner_mismatch",
    "Registry digest differs from the scanned archive": "registry_digest_conflict",
    "Registry publication outcome is uncertain": "registry_write_uncertain",
    "Registry copy budget expired": "command_timeout",
    "Runtime canary cleanup incomplete; retain the campaign receipt": "runtime_cleanup_incomplete",
    "Registry write is not visible yet": "registry_not_visible",
}
REASONS = frozenset({"registry_denied", "build_failed", "command_failed", "build_timeout", "command_timeout",
                     "scan_refused", "registry_read_failed", "validation_refused", "io_failed", "unexpected_error",
                     "reservation_superseded", *REFUSAL_REASONS.values()})
REGISTRY_STATUS = re.compile(
    r"\b(?:(?:response\s+)?status(?:\s*code)?|invalid status code from registry|http status)"
    r"(?:\s*:\s*|\s+)([1-5][0-9]{2})\b", re.IGNORECASE)
COPY_PHASES = (
    ("blob_upload_start", "initiating layer upload"),
    ("blob_upload", "uploading layer"),
    ("blob_reuse", "checking whether a blob|trying to reuse blob|mounting sha256:"),
    ("digest_preservation", "would require changing layer representation|would not match destination reference"),
    ("manifest_write", "writing manifest|uploading manifest"),
    ("config_copy", "copying (?:image )?config"),
    ("blob_copy", "copying (?:image )?(?:blob|layer)"),
    ("source_read", "reading manifest|determining manifest mime type|initializing image from source|"
                    "initializing source|reading blob|loading index|unpacking (?:the )?(?:archive|tarball)"),
    ("connect", "pinging container registry|getting bearer token"),
)
COPY_REASONS = (
    ("source_policy", r"source image rejected|signature verification failed"),
    ("archive_invalid", r"archive/tar:|invalid tar header|unsupported tar"),
    ("manifest_invalid", r"\b(?:manifest[ _]invalid|manifest_blob_unknown)\b"),
    ("digest_mismatch", r"digest (?:verification failed|mismatch|invalid)"),
    ("blob_unknown", r"\bblob[ _]unknown\b"),
    ("reference_invalid", r"invalid reference format|TAG_INVALID|NAME_INVALID"),
    ("insufficient_space", r"no space left on device|disk quota exceeded"),
    ("permission_denied", r"permission denied|operation not permitted"),
    ("input_missing", r"no such file or directory"),
    ("transport_incomplete", r"unexpected EOF|connection reset by peer|broken pipe"),
    ("transport_timeout", r"i/o timeout|TLS handshake timeout|context deadline exceeded"),
)
COPY_MILESTONES = (
    ("manifest", rb"Writing manifest to image destination"),
    ("config", rb"Copying config sha256:[0-9a-f]{64}"),
    ("layers", rb"Copying blob sha256:[0-9a-f]{64}"),
    ("signatures", rb"Getting image source signatures"),
)


class ImageCommandRefusal(Refusal):
    def __init__(self, args, stderr, *, stdout=b"", returncode=1):
        # Match only diagnostic tokens; never interpolate remote text or argv.
        registry = len(args) > 1 and args[0] == "skopeo" and args[1] in {"login", "copy", "inspect"}
        diagnostic = stderr.decode("utf-8", errors="replace") if isinstance(stderr, bytes) else ""
        statuses = REGISTRY_STATUS.findall(diagnostic)
        status = next((code for code in statuses if code in {"401", "403"}), None)
        denied = status or re.search(r"\b(?:UNAUTHORIZED|DENIED|permission_denied|write_package)\b",
                                     diagnostic, re.IGNORECASE)
        self.reason = "registry_denied" if registry and denied else "command_failed"
        self.registry_failure = None
        if self.reason == "registry_denied":
            operation = args[1]
            if operation == "copy" and len(args) >= 4:
                if args[-2].startswith("docker://"):
                    operation = "pull"
                elif args[-1].startswith("docker://"):
                    operation = "push"
            self.registry_failure = (operation, status or "unknown")
        self.copy_failure = None
        self.web_copy_failure = None
        if registry and args[1] == "copy" and len(args) >= 4 and args[-1].startswith("docker://"):
            phase = next((name for name, pattern in COPY_PHASES
                          if re.search(pattern, diagnostic, re.IGNORECASE)), "unknown")
            http_status = statuses[0] if statuses and len(set(statuses)) == 1 else "unknown"
            reason = next((name for name, pattern in COPY_REASONS
                           if re.search(pattern, diagnostic, re.IGNORECASE)),
                          "terminated_by_signal" if returncode < 0 else "unknown")
            lines = stdout.splitlines()
            milestone = next((name for name, pattern in COPY_MILESTONES
                              if any(re.fullmatch(pattern, line) for line in lines)), "none")
            self.copy_failure = phase, http_status, reason, milestone
        super().__init__("Image operation failed; captured output is private")


def skopeo_version(runner):
    try:
        output = runner(["skopeo", "--version"], timeout=5)
    except (Refusal, OSError, subprocess.TimeoutExpired):
        return "unknown"
    match = re.fullmatch(rb"skopeo version ([0-9]{1,4}\.[0-9]{1,4}\.[0-9]{1,4})"
                         rb"(?: \(commit: [0-9a-f]{7,40}\))?\s*", output)
    return match[1].decode("ascii") if match else "unknown"


class PublicationDiagnostics:
    def __init__(self):
        self.stage = "validate"
        self.reason = "unexpected_error"
        self.site = None
        self.scan_evidence = None
        self.web_copy_failure = None

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
            return REFUSAL_REASONS.get(str(error), "validation_refused")
        if isinstance(error, OSError):
            return "io_failed"
        return "unexpected_error"


def run_publication(operation, *, on_superseded=None, component=None, outcome=None):
    """Run one publication privately; on failure, record its fixed stage token in `outcome`."""
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
            diagnostics.site = error_site(error)
            if diagnostics.stage == "registry_copy" and isinstance(error, ImageCommandRefusal):
                diagnostics.web_copy_failure = error.web_copy_failure
            if outcome is not None and diagnostics.reason == "registry_denied":
                if isinstance(error, ImageCommandRefusal):
                    outcome["registry_failure"] = error.registry_failure
                elif isinstance(error.__cause__, urllib.error.HTTPError):
                    outcome["registry_failure"] = ("read", str(error.__cause__.code))
            raise

    status = run_private(guarded, None, subject="publication", explain=False)
    if not status and diagnostics.reason == "reservation_superseded":
        print(f"SKIP publication stage={diagnostics.stage} reason=reservation_superseded")
    if status:
        # No exception text, paths, identifiers or scanner findings cross here:
        # only fixed tokens and the public source line that failed.
        stage = diagnostics.stage if diagnostics.stage in STAGES else "validate"
        reason = diagnostics.reason if diagnostics.reason in REASONS else "unexpected_error"
        if outcome is not None:
            outcome["stage"] = stage
        report("publication", reason, subject="publication", stage=stage, site=diagnostics.site,
               component=component)
        if diagnostics.web_copy_failure is not None:
            role, phase, http_status, reason, milestone, version = diagnostics.web_copy_failure
            print(f"Web registry copy failed image_role={role} skopeo_phase={phase} "
                  f"skopeo_version={version} http_status={http_status} "
                  f"skopeo_reason={reason} skopeo_milestone={milestone}", flush=True)
        if diagnostics.scan_evidence is not None:
            print("SCAN_EVIDENCE " + json.dumps(diagnostics.scan_evidence, sort_keys=True))
    return status
