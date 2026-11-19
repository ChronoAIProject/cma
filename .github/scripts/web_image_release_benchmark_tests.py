"""Actual archive regressions for the shared web asset payload."""
import contextlib
import io
import json
from pathlib import Path
import tarfile
import tempfile
import unittest

from web_image_release_benchmark import asset_inventory, PREFIXES
import scan_image_secrets
from tests.fixtures import docker_archive, tar_bytes


class AssetLayoutTests(unittest.TestCase):
    def setUp(self):
        temporary = tempfile.TemporaryDirectory()
        self.addCleanup(temporary.cleanup)
        self.root = Path(temporary.name)
        self.archive = self.root / 'image.tar'
        self.current, self.retained = (p + 'fixture.js' for p in PREFIXES)
        self.payload = b'/* public fixture */\n'

    def image(self, entries):
        docker_archive(self.archive, [tar_bytes(entries)])
        return asset_inventory(self.archive)

    def test_duplicate_and_linked_layouts_have_identical_content(self):
        before, before_assets = self.image([(self.current, self.payload), (self.retained, self.payload)])
        after, after_assets = self.image([(self.current, self.payload),
                                         (self.retained, (tarfile.LNKTYPE, self.current))])
        self.assertEqual(before_assets, after_assets)
        self.assertEqual(before['asset_content_sha256'], after['asset_content_sha256'])
        self.assertEqual(before['asset_payload_bytes'], 2 * len(self.payload))
        self.assertEqual(after['asset_payload_bytes'], len(self.payload))
        self.assertEqual(after['hardlinks'], 1)

    def test_missing_or_changed_current_assets_fail(self):
        for entries in ([(self.retained, self.payload)],
                        [(self.current, self.payload), (self.retained, b'changed')]):
            with self.subTest(entries=len(entries)), self.assertRaises(ValueError):
                self.image(entries)

    def test_symlinks_outside_targets_and_link_chains_fail(self):
        for kind, target in ((tarfile.SYMTYPE, self.current),
                             (tarfile.LNKTYPE, '/etc/unknown'),
                             (tarfile.LNKTYPE, self.retained)):
            with self.subTest(kind=kind, target=target), self.assertRaises(ValueError):
                self.image([(self.current, self.payload), (self.retained, (kind, target))])

    def test_shared_payload_is_still_scanned_and_new_content_blocks(self):
        payload = b'-----BEGIN ' + b'PRIVATE KEY-----\nsynthetic scanner fixture\n'
        docker_archive(self.archive, [tar_bytes([(self.current, payload),
                      (self.retained, (tarfile.LNKTYPE, self.current))])])
        out = self.root / 'scan'
        with contextlib.redirect_stdout(io.StringIO()), contextlib.redirect_stderr(io.StringIO()):
            result = scan_image_secrets.main([str(self.archive), '--output', str(out),
                                             '--blocking-rules', 'credential'])
        self.assertEqual(result, 1)
        report = json.loads((out / 'REPORT.json').read_text())
        self.assertTrue(report['complete'])
        self.assertTrue(any(row['rule'] == 'content.private-key-header'
                            for row in report['blocking_findings']))


if __name__ == '__main__':
    unittest.main()
