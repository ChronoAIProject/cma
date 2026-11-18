#!/usr/bin/env python3
"""Fail-closed release publication ordering and archive-identity regressions."""
import io
import os
import subprocess
import tempfile
import unittest
from pathlib import Path
from unittest.mock import Mock, patch

import release_image_publication as publication

REVISION = "a" * 40


class PublicationTests(unittest.TestCase):
    def setUp(self):
        self.temporary = tempfile.TemporaryDirectory()
        self.addCleanup(self.temporary.cleanup)
        self.root = Path(self.temporary.name).resolve()
        self.archive = self.root / "candidate.oci.tar"
        self.archive.write_bytes(b"opaque archive fixture; scanner is injected")
        context = patch.dict(os.environ, {"SKOPEO_OUTPUT_DIR": str(self.root),
                                         "SKOPEO_COMMAND": "pinned-skopeo", "SKOPEO_AUTHFILE": "/auth/config.json"})
        context.start()
        self.addCleanup(context.stop)

    def publish(self, scanner, runner, **changes):
        values = {"archive": self.archive, "component": "web", "version": "v1.2.3", "revision": REVISION,
                  "repository": "ghcr.io/chronoaiproject/cma-web", "scanner": scanner, "run_command": runner}
        values.update(changes)
        return publication.publish_archive(**values)

    def test_scan_precedes_both_digest_preserving_publications(self):
        calls = []
        def scan(archive, output):
            self.assertEqual(archive, self.archive)
            self.assertEqual(output, self.root / "secret-scan")
            calls.append(("scan",))
        def runner(*args):
            calls.append(args)
            return '{"schemaVersion":2}'
        result = self.publish(scan, runner)
        self.assertEqual(result, publication.checksum(self.archive))
        self.assertEqual(len(calls), 7)
        self.assertEqual(calls[0], ("scan",))
        self.assertTrue(calls[1][0].endswith("/assert-release-image-tags-absent.sh"))
        for call, tag in zip(calls[3::2], ("v1.2.3", REVISION[:7])):
            self.assertEqual(call, ("pinned-skopeo", "copy", "--all", "--preserve-digests", "--authfile",
                                    "/auth/config.json", "oci-archive:/output/candidate.oci.tar",
                                    f"docker://ghcr.io/chronoaiproject/cma-web:{tag}"))

    def test_scan_failure_or_error_blocks_all_publication(self):
        for failure in (publication.Refusal("findings"), OSError("scanner unavailable")):
            runner = Mock()
            with self.subTest(failure=type(failure).__name__), self.assertRaises(type(failure)):
                self.publish(Mock(side_effect=failure), runner)
            runner.assert_not_called()

    def test_archive_substitution_after_scan_fails_closed(self):
        runner = Mock()
        def changed(_archive, _output):
            self.archive.write_bytes(b"different image")
        with self.assertRaisesRegex(publication.Refusal, "changed"):
            self.publish(changed, runner)
        runner.assert_not_called()

    def test_existing_tag_or_failed_push_stops_following_writes(self):
        for fail_at in (1, 2):
            calls = []
            def fail(*args, fail_at=fail_at, calls=calls):
                calls.append(args)
                if len(calls) == fail_at:
                    raise publication.Refusal("publication stopped")
                return '{"schemaVersion":2}'
            with self.subTest(fail_at=fail_at), self.assertRaises(publication.Refusal):
                self.publish(Mock(), fail)
            self.assertEqual(len(calls), fail_at)

    def test_untrusted_inputs_fail_before_scan(self):
        for fields in ({"component": "other"}, {"version": "latest"}, {"revision": "main"},
                       {"repository": "ghcr.io/chronoaiproject/cma-production"},
                       {"archive": self.root / "missing.tar"}):
            scanner, runner = Mock(), Mock()
            with self.subTest(fields=fields), self.assertRaises(publication.Refusal):
                self.publish(scanner, runner, **fields)
            scanner.assert_not_called()
            runner.assert_not_called()

    def test_cli_uses_shared_gate_and_redacts_unexpected_errors(self):
        args = [str(self.archive), "web", "v1.2.3", REVISION, "ghcr.io/chronoaiproject/cma-web"]
        with patch.object(publication, "publish_archive") as publish, patch("sys.stdout", new_callable=io.StringIO) as output:
            self.assertEqual(publication.main(args), 0)
            self.assertIs(publish.call_args.kwargs["scanner"], publication.scan_archive)
            for failure in (publication.Refusal("safe failure"), publication.ScanRefusal("scan refused"), OSError("private")):
                publish.side_effect = failure
                self.assertEqual(publication.main(args), 1)
            self.assertEqual(publication.main([]), 1)
            self.assertNotIn("private", output.getvalue())

    def test_command_errors_are_redacted(self):
        with patch.object(publication.subprocess, "run", return_value=Mock(returncode=0, stdout="ok")):
            self.assertEqual(publication.command("tool"), "ok")
        for failure in (OSError("private"), subprocess.TimeoutExpired("tool", 900)):
            with patch.object(publication.subprocess, "run", side_effect=failure), self.assertRaises(publication.Refusal):
                publication.command("tool")
        with patch.object(publication.subprocess, "run", return_value=Mock(returncode=1, stderr="private")), \
             self.assertRaises(publication.Refusal):
            publication.command("tool")


if __name__ == "__main__":
    unittest.main()
