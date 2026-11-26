"""A single public ref advertisement fences automatic deployment receipts."""
import fnmatch
import subprocess
import unittest
from unittest.mock import Mock

from staging_cd_common import Refusal
from staging_cd_receipts_tests import fixture_receipt
from staging_cd_versions import DEPLOYABLES
from staging_cd_watermark import SUPERSEDED, SupersededVersions, require_current_versions


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
        # Only final tags are read; a pending reservation can never withhold a receipt.
        self.assertEqual(runner.call_args.args[0][-2:], ["refs/tags/cma-web/v*", "refs/tags/cma-event-gateway/v*"])
        runner.return_value.stdout = "\n".join([*lines, "b" * 40 + "\trefs/tags/cma-web/v0.10.2"])
        with self.assertRaisesRegex(Refusal, SUPERSEDED):
            require_current_versions(receipts, runner=runner)

    def test_every_lane_ignores_a_pending_successor_but_not_a_higher_final(self):
        receipts = {key: fixture_receipt(component=key) for key in DEPLOYABLES}
        current = ["refs/tags/" + receipt["tag"] for receipt in receipts.values()]

        def advertise(*names):
            refs = current + ["refs/tags/" + name for name in names]
            def runner(args, **_kwargs):
                lines = ["a" * 40 + "\t" + ref for ref in refs if any(fnmatch.fnmatchcase(ref, p) for p in args)]
                return subprocess.CompletedProcess(args, 0, "\n".join(lines), "")
            return runner

        pending = [name + "/pending/v0.10.4" for name in DEPLOYABLES.values()]
        require_current_versions(receipts, runner=advertise(*pending))
        for key, name in DEPLOYABLES.items():
            newer = name + ("/v0.10.3" if key == "runtime" else "/v0.10.2")
            with self.subTest(component=key):
                with self.assertRaises(SupersededVersions) as error:
                    require_current_versions(receipts, runner=advertise(*pending, newer))
                self.assertEqual(error.exception.components, {key})

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
