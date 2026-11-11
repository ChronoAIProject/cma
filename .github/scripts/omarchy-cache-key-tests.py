#!/usr/bin/env python3
"""Cache identity must follow all pinned inputs without following app changes."""

import importlib.util
import json
from pathlib import Path
import subprocess
import sys
import tempfile
import unittest

SCRIPT = Path(__file__).with_name("omarchy-cache-key.py")
SPEC = importlib.util.spec_from_file_location("omarchy_cache_key", SCRIPT)
CACHE = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(CACHE)


class CacheIdentityTests(unittest.TestCase):
    def setUp(self):
        self.temporary = tempfile.TemporaryDirectory()
        self.addCleanup(self.temporary.cleanup)
        self.root = Path(self.temporary.name)
        self.dockerfile = self.root / CACHE.DOCKERFILE
        self.dockerfile.parent.mkdir(parents=True)
        self.recipe = (
            "FROM archlinux:base@sha256:" + "a" * 64 + " AS omarchy-dependencies\n"
            "COPY install.sh graphics.patch /tmp/\n"
            "RUN bash /tmp/install.sh\n"
            "FROM omarchy-dependencies AS omarchy\n"
            "COPY current-runtime /usr/local/bin/runtime\n"
        )
        self.dockerfile.write_text(self.recipe)
        (self.root / "install.sh").write_text("verify pinned source; compile\n")
        (self.root / "graphics.patch").write_text("pinned graphics changes\n")

    def identity(self):
        return CACHE.scope(CACHE.manifest(self.root))

    def test_every_copied_file_changes_identity(self):
        original = self.identity()
        for name in ("install.sh", "graphics.patch"):
            with self.subTest(name=name):
                path = self.root / name
                content = path.read_bytes()
                path.write_bytes(content + b"changed\n")
                self.assertNotEqual(original, self.identity())
                path.write_bytes(content)
        self.assertEqual(original, self.identity())

    def test_base_recipe_and_copy_destination_change_identity(self):
        original = self.identity()
        for changed in (self.recipe.replace("a" * 64, "b" * 64),
                        self.recipe.replace("RUN bash", "RUN env bash"),
                        self.recipe.replace("/tmp/\n", "/build/\n")):
            with self.subTest(recipe=changed):
                self.dockerfile.write_text(changed)
                self.assertNotEqual(original, self.identity())

    def test_application_changes_do_not_recompile_pinned_graphics(self):
        original = self.identity()
        (self.root / "current-runtime").write_bytes(b"new supervisor")
        self.dockerfile.write_text(self.recipe + "LABEL revision=new-revision\n")
        self.assertEqual(original, self.identity())

    def test_missing_or_non_regular_inputs_fail_closed(self):
        path = self.root / "graphics.patch"
        path.unlink()
        with self.assertRaises(ValueError):
            self.identity()
        path.mkdir()
        with self.assertRaises(ValueError):
            self.identity()
        path.rmdir()
        path.symlink_to(self.root / "install.sh")
        with self.assertRaises(ValueError):
            self.identity()

    def test_external_or_dynamic_copy_inputs_are_rejected(self):
        for instruction in ("COPY --from=untrusted /source /tmp/", "COPY $INPUT /tmp/",
                            "COPY ../outside /tmp/"):
            with self.subTest(instruction=instruction):
                self.dockerfile.write_text(self.recipe.replace("COPY install.sh graphics.patch /tmp/", instruction))
                with self.assertRaises(ValueError):
                    self.identity()

    def test_unreviewed_stage_instructions_fail_closed(self):
        self.dockerfile.write_text(self.recipe.replace("RUN bash", "ARG UNTRACKED\nRUN bash"))
        with self.assertRaises(ValueError):
            self.identity()

    def test_missing_duplicate_or_unpinned_stage_fails_closed(self):
        for changed in (self.recipe.replace(" AS omarchy-dependencies", " AS other"),
                        self.recipe + self.recipe,
                        self.recipe.replace("archlinux:base@sha256:" + "a" * 64, "archlinux:base")):
            with self.subTest(recipe=changed):
                self.dockerfile.write_text(changed)
                with self.assertRaises(ValueError):
                    self.identity()

    def test_real_recipe_emits_verifiable_input_evidence(self):
        evidence = self.root / "evidence" / "cache-inputs.json"
        result = subprocess.run([sys.executable, str(SCRIPT), str(evidence)],
                                check=True, capture_output=True, text=True)
        value = json.loads(evidence.read_text())
        recorded_scope = value.pop("scope")
        self.assertEqual(recorded_scope, CACHE.scope(value))
        self.assertEqual(result.stdout, "scope=" + recorded_scope + "\n")
        self.assertEqual(value["platform"], "linux/amd64")
        self.assertIn("api/crates/layer4-runtime/src/omarchy/build-hyprland.sh", value["inputs"])
        self.assertIn("api/crates/layer4-runtime/src/omarchy/hyprland-shm.patch", value["inputs"])
        self.assertNotIn("api/crates/layer4-runtime/src/omarchy/session.sh", value["inputs"])


if __name__ == "__main__":
    unittest.main()
