"""Warm detection preserves full physical coverage and current blocking policy."""
import copy
import json
import os
import unittest
from types import SimpleNamespace
from unittest.mock import patch

from image_secrets.backend import Gitleaks
from image_secrets.cache import MAX_INPUTS, LayerCache, fingerprint
from image_secrets.common import ScanError, digest_bytes
from tests import test_policy_a, test_scanner
from tests.fixtures import docker_archive, fake_key, oci_archive, tar_bytes


class LayerCacheTests(unittest.TestCase):
    setUp = test_scanner.ScannerTests.setUp
    tearDown = test_scanner.ScannerTests.tearDown
    scan = test_scanner.ScannerTests.scan

    def options(self, source=None, context="a" * 64):
        options = ["--detector-cache-context", context, "--detector-cache-output", str(self.root / "cache.json")]
        if source is not None:
            options += ["--detector-cache-input", str(source)]
        return options

    def paired(self, archive, expected):
        calls = []
        original = Gitleaks.detect

        def observed(backend, stage, locations, findings, **kwargs):
            calls.append(stage.name)
            return original(backend, stage, locations, findings, **kwargs)

        with patch.object(Gitleaks, "detect", observed):
            cold, _ = self.scan(archive, expected, self.options())
            cold_calls = calls[:]
            calls.clear()
            warm, _ = self.scan(archive, expected, self.options(self.root / "cache.json"))
        for field in ("findings", "allowlisted", "inventory", "scanned_bytes", "status", "counts", "policy"):
            self.assertEqual(cold[field], warm[field], field)
        self.assertIn("layer", cold_calls)
        self.assertNotIn("layer", calls)
        self.assertIn("config", calls)
        self.assertIn("metadata", calls)
        return cold, warm

    def test_clean_layer_reuse_keeps_coverage_and_skips_only_detector(self):
        archive = self.root / "clean.tar"
        oci_archive(archive, [tar_bytes([("app/file", b"ordinary content\n")])])
        cold, warm = self.paired(archive, 0)
        self.assertEqual(cold["detector_cache"]["misses"], 1)
        self.assertEqual(warm["detector_cache"]["hits"], 1)

    def test_real_findings_replay_and_nested_secret_still_block(self):
        nested = tar_bytes([("inner/key", fake_key())])
        archive = self.root / "secret.tar"
        docker_archive(archive, [tar_bytes([("nested.tar", nested)])])
        _, warm = self.paired(archive, 1)
        self.assertTrue(any(f["rule"].startswith("gitleaks.") for f in warm["findings"]))
        text = (self.root / "cache.json").read_text()
        self.assertNotIn("PRIVATE KEY", text)
        self.assertNotIn("nested.tar", text)

    def test_replayed_elf_findings_bind_to_original_file_bytes(self):
        archive = self.root / "binary.tar"
        binary = bytes.fromhex("7f454c4600ff") + fake_key()
        docker_archive(archive, [tar_bytes([("lib/example.so", binary)])])
        _, warm = self.paired(archive, 1)
        rows = [row for row in warm["findings"] if row["rule"].startswith("gitleaks.")]
        self.assertTrue(rows)
        for row in rows:
            self.assertEqual(row["file_sha256"], digest_bytes(binary))
            self.assertNotIn("line", row)
            self.assertNotIn("_file_sha256", row)

    def test_changed_layer_is_scanned_but_identical_layer_hits(self):
        archive = self.root / "change.tar"
        fixed = tar_bytes([("fixed", b"stable\n")])
        docker_archive(archive, [fixed])
        self.scan(archive, 0, self.options())
        docker_archive(archive, [fixed, tar_bytes([("new", fake_key())])])
        report, _ = self.scan(archive, 1, self.options(self.root / "cache.json"))
        self.assertEqual(report["detector_cache"]["hits"], 1)
        self.assertEqual(report["detector_cache"]["misses"], 1)

    def test_config_and_history_are_fresh_on_layer_hit(self):
        archive = self.root / "config.tar"
        layer = tar_bytes([("fixed", b"stable\n")])
        docker_archive(archive, [layer])
        self.scan(archive, 0, self.options())
        docker_archive(archive, [layer], {"config": {"Env": ["PASSWORD=" + os.urandom(32).hex()]}})
        report, _ = self.scan(archive, 1, self.options(self.root / "cache.json"))
        self.assertEqual(report["detector_cache"]["hits"], 1)
        self.assertTrue(any(f["scope"] == "config" for f in report["findings"]))

    def test_reordered_repeated_layers_rebind_physical_occurrences(self):
        archive = self.root / "repeat.tar"
        secret, clean = tar_bytes([("key", fake_key())]), tar_bytes([("clean", b"hello")])
        docker_archive(archive, [secret, clean])
        self.scan(archive, 1, self.options())
        docker_archive(archive, [clean, secret, secret])
        warm, _ = self.scan(archive, 1, self.options(self.root / "cache.json"))
        cold, _ = self.scan(archive, 1)
        self.assertEqual(warm["findings"], cold["findings"])
        self.assertEqual(warm["scanned_bytes"], cold["scanned_bytes"])
        self.assertEqual(warm["detector_cache"]["hits"], 3)

    def test_policy_context_and_limits_invalidate_reuse(self):
        archive = self.root / "policy.tar"
        docker_archive(archive, [tar_bytes([("file", b"ordinary\n")])])
        self.scan(archive, 0, self.options())
        report, _ = self.scan(archive, 0, self.options(self.root / "cache.json", "b" * 64))
        self.assertEqual(report["detector_cache"]["hits"], 0)
        report, _ = self.scan(archive, 0, self.options(self.root / "cache.json", "b" * 64) + ["--max-entries", "999999"])
        self.assertEqual(report["detector_cache"]["hits"], 0)

    def test_real_classification_blocks_after_cached_replay(self):
        archive = self.root / "real.tar"
        docker_archive(archive, [tar_bytes([("public.pem", b"synthetic noncredential content")])])
        report, _ = self.scan(archive, 1, ["--blocking-rules=credential"])
        packet = test_policy_a.PolicyATests.packet(self, report, "REAL")
        path = self.root / "classification.json"
        path.write_text(json.dumps(packet))
        policy = ["--blocking-rules=credential", "--classifications", str(path)]
        self.scan(archive, 1, self.options() + policy)
        warm, _ = self.scan(archive, 1, self.options(self.root / "cache.json") + policy)
        self.assertEqual(warm["detector_cache"]["hits"], 1)
        self.assertTrue(all(f["classification"] == "REAL" for f in warm["blocking_findings"]))

    def test_scanner_version_source_binary_rules_and_policy_bind_identity(self):
        scanner = test_scanner.cli.HERE
        backend = SimpleNamespace(executable="gitleaks", version="8.30.1")
        def identity(**changes):
            return fingerprint(scanner, changes.get("backend", backend), changes.get("version", "1.0.3"),
                               "a" * 64, changes.get("limits", [100, 100, 100]), changes.get("policy", {}))
        original = identity()
        for changed in ({"version": "1.0.4"}, {"backend": SimpleNamespace(executable="gitleaks", version="8.31.0")},
                        {"limits": [101, 100, 100]}, {"policy": {"catalog": "b" * 64}}):
            self.assertNotEqual(original, identity(**changed))
        from image_secrets.cache import digest_file
        for suffix in ("gitleaks.toml", "image_secrets/backend.py", "gitleaks"):
            def changed_digest(path, suffix=suffix):
                return "b" * 64 if str(path).endswith(suffix) else digest_file(path)
            with self.subTest(file=suffix), patch("image_secrets.cache.digest_file", changed_digest):
                self.assertNotEqual(original, identity())
    def test_missing_or_changed_coverage_refuses_matching_record(self):
        archive = self.root / "coverage.tar"
        docker_archive(archive, [tar_bytes([("file", b"ordinary\n")])])
        self.scan(archive, 0, self.options())
        original = json.loads((self.root / "cache.json").read_text())
        for mutate in (lambda r: r["coverage"].pop(), lambda r: r["coverage"][0].update(size=99)):
            document = copy.deepcopy(original)
            mutate(next(iter(document["layers"].values())))
            (self.root / "cache.json").write_text(json.dumps(document))
            report, _ = self.scan(archive, 2, self.options(self.root / "cache.json"))
            self.assertFalse(report["complete"])

    def test_corruption_duplicate_keys_and_incomplete_detection_fail_closed(self):
        archive = self.root / "broken.tar"
        docker_archive(archive, [tar_bytes([("file", b"ordinary\n")])])
        cache = self.root / "cache.json"
        for data in ('{', '{"schema":1,"schema":2}', '[]'):
            cache.write_text(data)
            self.scan(archive, 2, self.options(cache))
        cache.unlink()
        with patch.object(Gitleaks, "detect", side_effect=ScanError("gitleaks-incomplete")):
            self.scan(archive, 2, self.options())
        self.assertFalse(cache.exists())

    def test_hit_does_not_bypass_image_byte_budget(self):
        archive = self.root / "budget.tar"
        docker_archive(archive, [tar_bytes([("file", b"x" * 10000)])])
        self.scan(archive, 0, self.options())
        report, _ = self.scan(archive, 2, self.options(self.root / "cache.json") + ["--max-bytes", "100"])
        self.assertFalse(report["complete"])

    def test_claimed_cached_diff_id_does_not_hide_changed_layer_bytes(self):
        archive = self.root / "changed-bytes.tar"
        original = tar_bytes([("file", b"ordinary content")])
        docker_archive(archive, [original])
        self.scan(archive, 0, self.options())
        changed = tar_bytes([("file", fake_key())])
        docker_archive(archive, [changed], {"rootfs": {"type": "layers", "diff_ids": ["sha256:" + digest_bytes(original)]}})
        report, _ = self.scan(archive, 2, self.options(self.root / "cache.json"))
        self.assertFalse(report["complete"])

    def test_cache_bounds_withhold_whole_record_not_partial_coverage(self):
        cache = LayerCache("a" * 64)
        coverage = [{"name": f"entry-{i:08d}.txt", "size": 0, "sha256": digest_bytes(b"")} for i in range(2)]
        with patch("image_secrets.cache.MAX_INPUTS", 1):
            cache.store("sha256:" + "b" * 64, coverage, [])
        self.assertEqual(cache.layers, {})
        self.assertEqual(cache.stats["withheld"], 1)
        self.assertGreater(MAX_INPUTS, 1)

    def test_mapping_hashes_each_finding_file_once(self):
        archive = self.root / "many.tar"
        value = b"\n".join(b'password="' + os.urandom(32).hex().encode() + b'"' for _ in range(20))
        docker_archive(archive, [tar_bytes([("many", value)])])
        from image_secrets.backend import digest_file
        calls = []

        def count(path):
            calls.append(str(path))
            return digest_file(path)

        with patch("image_secrets.backend.digest_file", count):
            report, _ = self.scan(archive, 1)
        self.assertGreater(len(report["findings"]), 10)
        self.assertEqual(len(calls), len(set(calls)))
