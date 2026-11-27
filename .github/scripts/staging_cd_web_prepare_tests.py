"""Verify the incumbent-based recipe with synthetic OCI archives."""

from dataclasses import replace
import json
from pathlib import Path
import subprocess
import sys
import tempfile
import unittest

from staging_cd_common import Refusal
from staging_cd_web_overlay import (
    BASE_LABEL, BUILD_INFO, FALLBACK, HTML, MANIFEST_LABEL, REPOSITORY, assemble,
    assemble_prepare, hashed,
)
from staging_cd_web_overlay_tests import fixture_archive, unpack_overlay


class WebPrepareTests(unittest.TestCase):
    def setUp(self):
        workspace = tempfile.TemporaryDirectory()
        self.addCleanup(workspace.cleanup)
        self.root = Path(workspace.name)

    def test_every_release_asset_resolves_across_prepare_and_promote(self):
        shared = {f"shared-{index}.js": b"unchanged" for index in range(50)}
        old = shared | {f"old-lazy-{index}.js": b"incumbent" for index in range(84)}
        new = shared | {f"new-lazy-{index}.js": b"candidate" for index in range(150)}
        history = [{"created_by": "COPY Web files"}]
        incumbent, original_config, original_html = fixture_archive(
            self.root, "a" * 40, "0.22.1", extra_assets=old, history=history,
        )
        candidate, _, _ = fixture_archive(self.root, "b" * 40, "0.22.2", extra_assets=new)
        retained, _, _ = fixture_archive(self.root, "c" * 40, "0.22.0")
        promote = assemble(incumbent, candidate, [retained], self.root, max_files=300, max_bytes=4096)["promote"]
        published = replace(candidate, archive=promote["archive"], digest=promote["digest"])

        result = assemble_prepare(incumbent, published, self.root, max_files=287, max_bytes=4096)

        recipe = result["prepare"]
        digest, config, prepared = unpack_overlay(recipe["archive"])
        _, _, promoted = unpack_overlay(promote["archive"])
        old.update({"index-a.js": b"release-a", "common.css": b"unchanged"})
        new.update({"index-b.js": b"release-b", "common.css": b"unchanged"})
        union = old | new
        self.assertEqual((len(old), len(new), len(union), result["asset_count"]), (136, 202, 287, 287))
        self.assertEqual(digest, recipe["digest"])
        self.assertEqual(result["candidate_digest"], published.digest)
        self.assertEqual(result["sentinels"]["incumbent"]["html_sha256"], hashed(original_html))
        labels = config["config"]["Labels"]
        self.assertEqual(labels.pop(BASE_LABEL), REPOSITORY + "@" + incumbent.digest)
        self.assertEqual(labels.pop(MANIFEST_LABEL), result["asset_manifest_sha256"])
        self.assertEqual(config["config"], original_config["config"])
        self.assertEqual(config["rootfs"]["diff_ids"][:-1], original_config["rootfs"]["diff_ids"])
        self.assertEqual(config["history"][:-1], history)
        self.assertNotIn(HTML, prepared)
        self.assertNotIn(BUILD_INFO, prepared)
        marker = prepared.pop(FALLBACK + "cma-asset-manifest-" + result["asset_manifest_sha256"] + ".json")
        self.assertEqual(hashed(marker), result["asset_manifest_sha256"])
        self.assertEqual(json.loads(marker), result["assets"])
        self.assertEqual(set(prepared), {FALLBACK + name for name in union})
        # Nginx checks the serving release's current catalog before its fallback layer.
        for name, expected in union.items():
            with self.subTest(path="/assets/" + name):
                self.assertEqual(old.get(name, prepared[FALLBACK + name]), expected)
                self.assertEqual(new.get(name, promoted[FALLBACK + name]), expected)
                self.assertEqual(result["assets"][name], {"bytes": len(expected), "sha256": hashed(expected)})

    def test_refuses_conflicting_assets_or_wrong_pin_before_creating_an_archive(self):
        incumbent, _, _ = fixture_archive(self.root, "a" * 40, "0.22.1")
        candidate, _, _ = fixture_archive(self.root, "b" * 40, "0.22.2", stylesheet=b"different")
        with self.assertRaisesRegex(Refusal, "immutable asset bytes"):
            assemble_prepare(incumbent, candidate, self.root, max_files=10, max_bytes=4096)
        with self.assertRaisesRegex(Refusal, "immutable release"):
            wrong = replace(candidate, digest="sha256:" + "c" * 64)
            assemble_prepare(incumbent, wrong, self.root, max_files=10, max_bytes=4096)
        self.assertFalse(list(self.root.glob("web-asset-prepare-*")))

    def test_prepare_is_deterministic_and_enforces_the_union_budget(self):
        incumbent, _, _ = fixture_archive(self.root, "a" * 40, "0.22.1")
        candidate, _, _ = fixture_archive(self.root, "b" * 40, "0.22.2")
        with self.assertRaisesRegex(Refusal, "approved budget"):
            assemble_prepare(incumbent, candidate, self.root, max_files=2, max_bytes=4096)
        self.assertFalse(list(self.root.glob("web-asset-prepare-*")))
        first = assemble_prepare(incumbent, candidate, self.root, max_files=3, max_bytes=4096)["prepare"]
        second = assemble_prepare(incumbent, candidate, self.root, max_files=3, max_bytes=4096)["prepare"]
        self.assertEqual(first["digest"], second["digest"])
        self.assertEqual(first["archive"].read_bytes(), second["archive"].read_bytes())

    def test_command_emits_a_digest_bound_catalog(self):
        incumbent, _, _ = fixture_archive(self.root, "a" * 40, "0.22.1")
        candidate, _, _ = fixture_archive(self.root, "b" * 40, "0.22.2")
        tool = Path(__file__).with_name("staging_cd_web_prepare.py")
        result = subprocess.run([
            sys.executable, "-B", str(tool),
            "--incumbent", str(incumbent.archive), incumbent.digest, incumbent.source, incumbent.version,
            "--candidate", str(candidate.archive), candidate.digest, candidate.source, candidate.version,
            "--output", str(self.root), "--max-files", "3", "--max-bytes", "4096",
        ], capture_output=True, check=True, text=True)
        receipt = json.loads(result.stdout)
        archive = Path(receipt["prepare"]["archive"])
        self.assertEqual(hashed(archive.read_bytes()), receipt["prepare"]["archive_sha256"])
        self.assertEqual(set(receipt["assets"]), {"index-a.js", "index-b.js", "common.css"})


if __name__ == "__main__":
    unittest.main()
