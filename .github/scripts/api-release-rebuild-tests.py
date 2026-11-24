#!/usr/bin/env python3
"""Exercise the release rebuild checker against a disposable Cargo workspace."""

import json
import os
from pathlib import Path
import subprocess
import sys
import tempfile
import unittest


CHECKER = Path(__file__).with_name("api-release-rebuild.py").resolve()


class ReleaseRebuildTests(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        temporary = tempfile.TemporaryDirectory(prefix="api-release-rebuild-")
        cls.addClassCleanup(temporary.cleanup)
        cls.root = Path(temporary.name)
        cls.environment = dict(os.environ, CARGO_TARGET_DIR=str(cls.root / "target"))
        (cls.root / "Cargo.toml").write_text(
            '[workspace]\nresolver = "2"\nmembers = ["service", "implementation", "sibling"]\n'
        )
        for name in ("service", "implementation", "sibling"):
            package = cls.root / name
            (package / "src").mkdir(parents=True)
            manifest = f'[package]\nname = "{name}"\nversion = "0.1.0"\nedition = "2021"\n'
            if name == "service":
                manifest += (
                    '[dependencies]\nimplementation = { path = "../implementation" }\n'
                    'sibling = { path = "../sibling" }\n'
                )
            (package / "Cargo.toml").write_text(manifest)
        (cls.root / "service/src/main.rs").write_text(
            'fn main() { println!("{}", implementation::value() + sibling::value()); }\n'
        )
        cls.implementation = cls.root / "implementation/src/lib.rs"
        cls.sibling = cls.root / "sibling/src/lib.rs"
        cls.implementation.write_text("pub fn value() -> u32 { 1 }\n")
        cls.sibling.write_text("pub fn value() -> u32 { 10 }\n")
        cls.cargo("generate-lockfile", "--offline")
        cls.metadata = cls.root / "metadata.json"
        cls.metadata.write_text(cls.cargo("metadata", "--locked", "--no-deps", "--format-version", "1"))
        cls.build("--release")
        cls.baseline = cls.root / "warm.jsonl"
        cls.baseline.write_text(cls.build("--release"))
        cls.implementation.write_text("pub fn value() -> u32 { 200 }\n")
        cls.edited = cls.root / "edited.jsonl"
        cls.edited.write_text(cls.build("--release"))
        cls.implementation.write_text("pub fn value() -> u32 { 3000 }\n")
        cls.sibling.write_text("pub fn value() -> u32 { 40000 }\n")
        cls.rebuilt_sibling = cls.root / "rebuilt-sibling.jsonl"
        cls.rebuilt_sibling.write_text(cls.build("--release"))
        cls.missing_artifact = cls.root / "missing-artifact.jsonl"
        cls.missing_artifact.write_text("".join(
            line for line in cls.edited.read_text().splitlines(keepends=True)
            if json.loads(line).get("target", {}).get("name") != "sibling"
        ))
        cls.build()
        cls.debug_baseline = cls.root / "debug-warm.jsonl"
        cls.debug_baseline.write_text(cls.build())
        cls.implementation.write_text("pub fn value() -> u32 { 500000 }\n")
        cls.debug_edited = cls.root / "debug-edited.jsonl"
        cls.debug_edited.write_text(cls.build())
        cls.changed_profile = cls.root / "release-with-debug-info.jsonl"
        cls.changed_profile.write_text(cls.build("--release", "--config", "profile.release.debug=1"))
        cls.changed_path = cls.root / "other-target-dir.jsonl"
        cls.changed_path.write_text(cls.build("--release", "--target-dir", str(cls.root / "other-target")))

        harness = ("test", "--release", "--locked", "--no-run", "--bin", "service", "--message-format=json")
        cls.cargo(*harness)
        cls.test_baseline = cls.root / "test-warm.jsonl"
        cls.test_baseline.write_text(cls.cargo(*harness))
        cls.implementation.write_text("pub fn value() -> u32 { 6000000 }\n")
        cls.test_edited = cls.root / "test-edited.jsonl"
        cls.test_edited.write_text(cls.cargo(*harness))

    @classmethod
    def cargo(cls, *args):
        result = subprocess.run(
            ["cargo", *args], cwd=cls.root, env=cls.environment, capture_output=True, text=True, timeout=90,
        )
        if result.returncode:
            raise RuntimeError(f"cargo {args[0]} (exit {result.returncode})\n{result.stderr}\n{result.stdout}")
        return result.stdout

    @classmethod
    def build(cls, *options):
        return cls.cargo("build", "--locked", "--bin", "service", "--message-format=json", *options)

    def check(self, edited, *args, baseline=None):
        return subprocess.run(
            [
                sys.executable, str(CHECKER), str(self.metadata), str(baseline or self.baseline), str(edited),
                "--implementation", "implementation:lib:implementation", "--binary", "service:bin:service",
                *args,
            ],
            capture_output=True, text=True, timeout=10,
        )

    def test_changed_implementation_rebuilds_binary_with_fresh_sibling(self):
        result = self.check(
            self.edited, "--fresh", "sibling:lib:sibling",
            "--extracted-implementation", "implementation:lib:implementation",
            "--extracted-fresh", "sibling:lib:sibling",
        )
        self.assertEqual(result.returncode, 0, result.stderr or result.stdout)
        report = json.loads(result.stdout)
        self.assertEqual(report["current"]["status"], "pass")
        self.assertEqual(report["extraction"]["status"], "pass")
        baseline = report["artifacts"]["baseline"]
        self.assertEqual(len(baseline), 3)
        self.assertTrue(all(row["fresh"] for row in baseline))
        members = set(json.loads(self.metadata.read_text())["workspace_members"])
        artifacts = report["artifacts"]["edited"]
        self.assertEqual({row["package_id"] for row in artifacts}, members)
        self.assertCountEqual(
            [(row["package"], row["kinds"], row["fresh"]) for row in artifacts],
            [
                ("implementation", ["lib"], False),
                ("service", ["bin"], False),
                ("sibling", ["lib"], True),
            ],
        )
        for phase in (baseline, artifacts):
            service = next(row for row in phase if row["package"] == "service")
            expected_name = "service.exe" if sys.platform == "win32" else "service"
            self.assertEqual(
                Path(service["executable"]).resolve(), self.root.resolve() / "target" / "release" / expected_name,
            )
            self.assertIs(service["profile"]["test"], False)

    def test_rejects_debug_or_test_executables(self):
        for baseline, edited in (
            (self.debug_baseline, self.debug_edited),
            (self.debug_baseline, self.edited),
            (self.test_baseline, self.test_edited),
        ):
            with self.subTest(baseline=baseline.name, edited=edited.name):
                result = self.check(edited, baseline=baseline)
                self.assertEqual(result.returncode, 1, result.stderr or result.stdout)
                errors = json.loads(result.stdout)["current"]["errors"]
                self.assertIn("expected an ordinary release executable for service:bin:service", errors)

    def test_rejects_different_release_outputs_or_compile_profiles(self):
        for edited, error in (
            (self.changed_path, "release executable path changed for service:bin:service"),
            (self.changed_profile, "target, features or Cargo profile changed for service:bin:service"),
        ):
            with self.subTest(edited=edited.name):
                result = self.check(edited)
                self.assertEqual(result.returncode, 1, result.stderr or result.stdout)
                self.assertIn(error, json.loads(result.stdout)["current"]["errors"])

    def test_absent_extraction_is_pending_even_if_current_comparison_fails(self):
        for edited, code, status in ((self.edited, 0, "pass"), (self.rebuilt_sibling, 1, "fail")):
            with self.subTest(edited=edited.name):
                result = self.check(
                    edited, "--fresh", "sibling:lib:sibling", "--extracted-implementation", "http:lib:http",
                )
                self.assertEqual(result.returncode, code, result.stderr or result.stdout)
                report = json.loads(result.stdout)
                self.assertEqual(report["current"]["status"], status)
                self.assertEqual(report["extraction"]["status"], "pending")
                self.assertEqual(report["extraction"]["missing_packages"], ["http"])
                if code:
                    self.assertEqual(report["current"]["errors"], ["expected sibling:lib:sibling to be fresh"])

    def test_required_artifact_missing_from_present_package_fails(self):
        result = self.check(
            self.missing_artifact, "--fresh", "sibling:lib:sibling",
            "--extracted-fresh", "sibling:lib:sibling",
        )
        self.assertEqual(result.returncode, 1, result.stderr or result.stdout)
        report = json.loads(result.stdout)
        for comparison in ("current", "extraction"):
            self.assertEqual(report[comparison]["status"], "fail")
            self.assertEqual(report[comparison]["errors"], ["missing artifact for sibling:lib:sibling"])

    def test_requires_successful_build_completion_on_both_sides(self):
        failed = self.root / "build-failed.jsonl"
        failed.write_text('{"reason":"build-finished","success":false}\n')
        for baseline, edited in ((failed, self.edited), (self.baseline, failed)):
            with self.subTest(baseline=baseline.name, edited=edited.name):
                result = self.check(edited, baseline=baseline)
                self.assertEqual(result.returncode, 2, result.stderr or result.stdout)
                self.assertIn("lacks one successful Cargo build completion", result.stderr)


if __name__ == "__main__":
    unittest.main()
