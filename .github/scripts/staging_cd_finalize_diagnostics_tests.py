"""Finalization errors expose only fixed stages and reasons."""
import contextlib
import io
import json
import os
from pathlib import Path
import subprocess
import sys
import tempfile
import unittest
from unittest.mock import Mock, patch

from staging_cd_common import Refusal
import staging_cd_finalize as finalization
from staging_cd_finalize_diagnostics import REASONS, run_finalization
from image_scanner_policy import MANIFEST_SHA256
from staging_cd_receipts import published_receipt
from staging_cd_reservations import next_reservation


class FinalizeDiagnosticsTests(unittest.TestCase):
    def observe(self, operation):
        output = io.StringIO()
        with contextlib.redirect_stdout(output):
            status = run_finalization(operation)
        return status, output.getvalue()

    def test_fixed_reasons_and_unknown_errors_withhold_private_text(self):
        cases = [(Refusal(message), reason) for message, reason in REASONS.items()]
        cases += [(Refusal("private\n::warning::fixture"), "validation_refused"),
                  (ValueError("private payload"), "unexpected_error"),
                  (OSError("private path"), "io_failed"),
                  (subprocess.TimeoutExpired("private command", 60), "command_timeout")]
        for error, reason in cases:
            with self.subTest(reason=reason):
                def operation(_masker, diagnostics):
                    diagnostics.mark("final_verify")
                    print("private response")
                    raise error
                self.assertEqual(self.observe(operation),
                                 (1, f"FAIL\nFAIL finalize stage=final_verify reason={reason}\n"))

    def test_success_and_real_entrypoint_keep_boundary(self):
        self.assertEqual(self.observe(lambda *_: True), (0, "PASS\n"))
        with patch.object(finalization, "run_finalization", return_value=23) as boundary:
            self.assertEqual(finalization.main(), 23)
        boundary.assert_called_once()

    def test_entrypoint_rejects_unfinalized_receipt_from_other_run_or_attempt(self):
        for run_id, attempt in ((2, 1), (1, 2)):
            with self.subTest(run_id=run_id, attempt=attempt), tempfile.TemporaryDirectory() as directory:
                reservation = next_reservation("web", "a" * 40, "b" * 40, 1, "patch", run_id, attempt, [], [])
                receipt = published_receipt(dict(reservation, schema="cma-image-release-v1", build="c" * 64),
                                            "sha256:" + "d" * 64, "e" * 64,
                                            {"detected": 0, "blocking": 0, "advisory": 0, "reviewed": 0, "allowlisted": 0}, scanner_identity=("1.0.3", MANIFEST_SHA256))
                path = Path(directory) / "receipts.json"
                path.write_text(json.dumps([receipt]))
                env = {"GITHUB_EVENT_NAME": "push", "GITHUB_RUN_ID": "1", "GITHUB_RUN_ATTEMPT": "1",
                       "CD_RECEIPTS_PATH": str(path), "GH_TOKEN": "fixture"}
                api = Mock()
                api.tags.return_value = []
                output = io.StringIO()
                with patch.dict(os.environ, env, clear=True), patch.object(finalization, "event_identity"), \
                        patch.object(finalization, "GitHub", return_value=api), \
                        patch.object(finalization, "Reservations") as store, contextlib.redirect_stdout(output):
                    self.assertEqual(finalization.main(), 1)
                self.assertEqual(output.getvalue(), "FAIL\nFAIL finalize stage=attempt reason=attempt_burned\n")
                store.return_value.finalize.assert_not_called()

    def test_untrusted_diagnostic_fields_are_not_printed(self):
        def operation(_masker, diagnostics):
            diagnostics.stage = "private-stage"
            diagnostics.reason = "private-reason"
            return False
        self.assertEqual(self.observe(operation), (1, "FAIL\nFAIL finalize stage=event reason=unexpected_error\n"))

    def test_native_and_child_output_is_suppressed(self):
        program = """
import os, subprocess, sys
from staging_cd_finalize_diagnostics import run_finalization
from staging_cd_common import Refusal
def operation(_masker, diagnostics):
    diagnostics.mark('cleanup_read')
    os.write(1, b'private native output')
    os.write(2, b'private native error')
    subprocess.run([sys.executable, '-c', "print('private child output')"], check=True)
    raise Refusal('Published reservation cleanup failed')
raise SystemExit(run_finalization(operation))
"""
        result = subprocess.run([sys.executable, "-B", "-c", program], cwd=Path(__file__).parent,
                                capture_output=True, text=True, check=False)
        self.assertEqual(result.returncode, 1)
        self.assertEqual(result.stdout, "FAIL\nFAIL finalize stage=cleanup_read reason=cleanup_unverified\n")
        self.assertEqual(result.stderr, "")


if __name__ == "__main__":
    unittest.main()
