"""A single public ref advertisement fences automatic deployment receipts."""
import subprocess
import unittest
from unittest.mock import Mock

from staging_cd_common import Refusal
from staging_cd_receipts_tests import fixture_receipt
from staging_cd_watermark import SUPERSEDED, require_current_versions


class WatermarkTests(unittest.TestCase):
    def test_components_have_independent_watermarks_and_the_read_has_no_credentials(self):
        receipts = {"web": fixture_receipt(), "eg": fixture_receipt(component="eg", version="0.11.1",
                                                                     previous="0.11.0")}
        lines = ["a" * 40 + "\trefs/tags/" + receipt["tag"] for receipt in receipts.values()]
        runner = Mock(return_value=subprocess.CompletedProcess([], 0, "\n".join(lines), ""))
        require_current_versions(receipts, runner=runner)
        runner.assert_called_once()
        self.assertNotIn("GH_TOKEN", runner.call_args.kwargs["env"])
        self.assertNotIn("HOME", runner.call_args.kwargs["env"])
        for suffix in ("v0.10.2", "pending/v0.10.2"):
            runner.return_value.stdout = "\n".join([*lines, "b" * 40 + "\trefs/tags/cma-web/" + suffix])
            with self.assertRaisesRegex(Refusal, SUPERSEDED):
                require_current_versions(receipts, runner=runner)

    def test_incomplete_malformed_and_failed_reads_cannot_authorize_deployment(self):
        receipts = {"web": fixture_receipt()}
        ref = "a" * 40 + "\trefs/tags/" + receipts["web"]["tag"]
        for data in ("", ref + "\n" + ref, ref.replace("0.10.1", "unknown"),
                     ref.replace("cma-web", "other"), ref.replace("a" * 40, "broken")):
            with self.subTest(data=data), self.assertRaises(Refusal):
                require_current_versions(receipts, runner=Mock(return_value=subprocess.CompletedProcess([], 0, data, "")))
        for runner in (Mock(side_effect=subprocess.TimeoutExpired("git", 10)),
                       Mock(return_value=subprocess.CompletedProcess([], 1, ref, ""))):
            with self.assertRaisesRegex(Refusal, "unavailable"):
                require_current_versions(receipts, runner=runner)
