"""Exercise the authenticated, read-only launcher boundary for Web publication."""

from pathlib import Path
import tempfile
import unittest
from unittest.mock import Mock

from staging_cd_clock import timestamp
from staging_cd_common import Refusal
from staging_cd_receipts import canonical
from staging_cd_web_continuity_tests import fixture_continuity, fixture_preimage
from staging_cd_web_preimage import read


class WebPreimageTests(unittest.TestCase):
    def setUp(self):
        temp = tempfile.TemporaryDirectory()
        self.addCleanup(temp.cleanup)
        self.root = Path(temp.name)
        source = "a" * 40
        event = self.root / "event.json"
        event.write_text(canonical({"ref": "refs/heads/develop", "after": source, "deleted": False,
                                    "repository": {"full_name": "ChronoAIProject/cma"}}))
        self.env = {
            "GITHUB_REPOSITORY": "ChronoAIProject/cma", "GITHUB_REF": "refs/heads/develop",
            "GITHUB_EVENT_NAME": "push", "GITHUB_EVENT_PATH": str(event), "GITHUB_SHA": source,
            "CD_NYXID_CLIENT_ID": "test-client-id", "CD_NYXID_CLIENT_SECRET": "test-client-secret",
            "GH_TOKEN": "test-registry-token", "CD_INFRA_TOKEN": "test-checkout-token",
            "RUNNER_TEMP": temp.name, "CD_STARTED_AT": timestamp(),
        }
        self.script = self.root / "staging" / "scripts" / "deploy_set.py"
        continuity = fixture_continuity()
        self.preimage = fixture_preimage(continuity["incumbent"], continuity["asset_manifest_sha256"])
        self.preimage["observed_at"] = self.env["CD_STARTED_AT"]

    def probe(self, runner):
        return read(self.env, Mock(), script_reader=lambda _: self.script, runner=runner,
                    tool_reader=lambda _: {"CD_TOOL_BIN": "/pinned/bin"})

    def test_passes_only_read_only_flags_and_captures_private_document(self):
        def runner(args, env, timeout, *, cwd, grace_seconds):
            self.assertEqual(args[1:7], ["-B", str(self.script), "--mode", "release", "--preflight-only", "--web-preimage"])
            destination = Path(args[7])
            self.assertEqual(args[8:], ["--started-at", self.env["CD_STARTED_AT"]])
            self.assertEqual(cwd, self.root)
            self.assertEqual((env["CD_NYXID_CLIENT_ID"], env["CD_NYXID_CLIENT_SECRET"]),
                             ("test-client-id", "test-client-secret"))
            self.assertTrue({"GH_TOKEN", "CD_INFRA_TOKEN", "GITHUB_EVENT_PATH"}.isdisjoint(env))
            self.assertEqual((destination.parent.stat().st_mode & 0o777), 0o700)
            self.assertTrue(0 < timeout <= 120)
            self.assertGreater(grace_seconds, 0)
            destination.write_text(canonical(self.preimage))
            destination.chmod(0o600)
            return 0

        self.assertEqual(self.probe(runner), self.preimage)
        self.assertFalse(list(self.root.glob("cma-web-preimage-*")))

    def test_unsuccessful_or_nonprivate_output_never_adopts_preimage(self):
        for case, expected in (("failed", "was not established"), ("symlink", "file is missing"),
                               ("mode", "not private"), ("size", "oversized")):
            def runner(args, *_args, **_kwargs):
                destination = Path(args[7])
                if case == "failed":
                    return 20
                if case == "symlink":
                    destination.symlink_to(Path(self.env["GITHUB_EVENT_PATH"]))
                else:
                    destination.write_text("x" * 4097 if case == "size" else canonical(self.preimage))
                    destination.chmod(0o644 if case == "mode" else 0o600)
                return 0

            with self.subTest(case=case), self.assertRaisesRegex(Refusal, expected):
                self.probe(runner)
