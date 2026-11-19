"""Closed finalization diagnostics preserve the private output boundary."""
import subprocess
import urllib.error

from staging_cd_common import Refusal
from staging_cd_runtime import run_private
from staging_cd_watermark import SUPERSEDED

STAGES = frozenset({"event", "artifact", "attempt", "receipt", "allocation", "registry_verify",
                    "final_read", "reservation_read", "tag_create", "tag_push", "final_verify",
                    "cleanup_verify", "cleanup_delete", "cleanup_read", "output"})
REASONS = {
    "Published receipt artifact is unavailable": "artifact_unavailable",
    "Prior-attempt final tag object differs": "recovery_oid_unverified",
    "A failed publication attempt burned this unfinalized version": "attempt_burned",
    "Publication receipts are reordered or duplicated": "receipt_order_invalid",
    "Resumed receipt changed": "resume_mismatch",
    "Published receipt differs from the allocation owner": "allocation_mismatch",
    "Final version requires the verified published digest": "digest_unverified",
    "This merge already has a different immutable release": "final_conflict",
    "Final tag conflicts with the published receipt": "final_conflict",
    "Final version lacks its allocation owner's reservation": "reservation_missing",
    "Publication reservation is missing": "reservation_missing",
    SUPERSEDED: "reservation_superseded",
    "Publication reservation is superseded and burned": "reservation_superseded",
    "Final annotation could not be created": "annotation_failed",
    "Final tag creation could not be verified": "final_unverified",
    "Final publication unexpectedly reported an up-to-date ref": "unexpected_existing_ref",
    "Pending deletion requires the verified matching final receipt": "cleanup_final_unverified",
    "Pending version belongs to another source": "cleanup_owner_mismatch",
    "Published reservation cleanup failed": "cleanup_unverified",
    "GitHub tag object differs from advertised ID": "final_conflict",
    "GitHub API request failed": "github_read_failed",
    "Develop ancestry could not be verified": "history_unverified",
    "Release source is not on verified develop first-parent history": "source_history_invalid",
    "Registry request failed": "registry_read_failed",
}


class FinalizeDiagnostics:
    def __init__(self):
        self.stage = "event"
        self.reason = "unexpected_error"

    def mark(self, stage):
        if stage not in STAGES:
            raise Refusal("Unknown finalization diagnostic stage")
        self.stage = stage

    def classify(self, error):
        if isinstance(error, Refusal):
            cause = error.__cause__
            if self.stage == "registry_verify" and isinstance(cause, urllib.error.HTTPError):
                return "registry_denied" if cause.code in (401, 403) else "registry_read_failed"
            return REASONS.get(str(error), "validation_refused")
        if isinstance(error, subprocess.TimeoutExpired):
            return "command_timeout"
        if isinstance(error, OSError):
            return "io_failed"
        return "unexpected_error"


def run_finalization(operation):
    diagnostics = FinalizeDiagnostics()

    def guarded(masker):
        try:
            return operation(masker, diagnostics)
        except Exception as error:
            diagnostics.reason = diagnostics.classify(error)
            raise

    status = run_private(guarded, None)
    if status:
        stage = diagnostics.stage if diagnostics.stage in STAGES else "event"
        allowed = set(REASONS.values()) | {"validation_refused", "unexpected_error", "registry_denied",
                                         "command_timeout", "io_failed"}
        reason = diagnostics.reason if diagnostics.reason in allowed else "unexpected_error"
        print(f"FAIL finalize stage={stage} reason={reason}")
    return status
