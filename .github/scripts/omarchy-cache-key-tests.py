"""Cache identity must follow all pinned inputs without following app changes."""

import importlib.util
import json
import subprocess
import sys
import tempfile
import unittest
from pathlib import Path

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
            "FROM archlinux:base@sha256:" + "a" * 64 + " AS omarchy-bootstrap\n"
            "COPY seed.sh /tmp/\n"
            "RUN bash /tmp/seed.sh /payload\n"
            "FROM scratch AS omarchy-base\n"
            "COPY --link --from=omarchy-bootstrap /payload/ /\n"
            "FROM omarchy-base AS omarchy-dependencies\n"
            "COPY install.sh graphics.patch /tmp/\n"
            "RUN bash /tmp/install.sh\n"
            "FROM omarchy-dependencies AS omarchy\n"
            "COPY current-runtime /usr/local/bin/runtime\n"
        )
        self.dockerfile.write_text(self.recipe)
        (self.root / "seed.sh").write_text("assemble signed base\n")
        (self.root / "install.sh").write_text("verify pinned source; compile\n")
        (self.root / "graphics.patch").write_text("pinned graphics changes\n")

    def identity(self):
        return CACHE.scope(CACHE.manifest(self.root))

    def test_every_copied_file_changes_identity(self):
        original = self.identity()
        for name in ("seed.sh", "install.sh", "graphics.patch"):
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

    def test_rejects_copying_the_old_filesystem_instead_of_the_package_root(self):
        self.dockerfile.write_text(self.recipe.replace(" /payload/ /\n", " / /\n"))
        with self.assertRaisesRegex(ValueError, "assembled package root"):
            self.identity()

    def test_historical_pinned_arch_recipe_still_resolves(self):
        _, dependencies = self.recipe.split("FROM omarchy-base AS omarchy-dependencies\n")
        pinned = "FROM archlinux:base@sha256:" + "a" * 64 + " AS omarchy-dependencies\n"
        self.dockerfile.write_text(pinned + dependencies)
        self.assertEqual(set(CACHE.manifest(self.root)["inputs"]), {"install.sh", "graphics.patch"})

    def test_unreviewed_stage_instructions_fail_closed(self):
        self.dockerfile.write_text(self.recipe.replace("RUN bash", "ARG UNTRACKED\nRUN bash"))
        with self.assertRaises(ValueError):
            self.identity()

    def test_pinned_stage_argument_default_changes_identity(self):
        pinned = "ARG SNAPSHOT=2026/10/22\nARG UNRELATED=1\n" + self.recipe.replace(
            "RUN bash /tmp/install.sh", "ARG SNAPSHOT\nRUN bash /tmp/install.sh \"${SNAPSHOT}\"")
        self.dockerfile.write_text(pinned)
        original = self.identity()
        self.assertEqual(CACHE.manifest(self.root)["arguments"], {"SNAPSHOT": "2026/10/22"})
        self.dockerfile.write_text(pinned.replace("UNRELATED=1", "UNRELATED=2"))
        self.assertEqual(original, self.identity())
        self.dockerfile.write_text(pinned.replace("SNAPSHOT=2026/10/22", "SNAPSHOT=2026/10/23"))
        self.assertNotEqual(original, self.identity())

    def test_stage_argument_without_pinned_global_default_fails_closed(self):
        for prefix, declaration in (("", "ARG SNAPSHOT"), ("ARG SNAPSHOT\n", "ARG SNAPSHOT"),
                                    ("ARG SNAPSHOT=1\n", "ARG SNAPSHOT=2")):
            with self.subTest(prefix=prefix, declaration=declaration):
                self.dockerfile.write_text(prefix + self.recipe.replace("RUN bash /tmp/seed.sh",
                                                                        declaration + "\nRUN bash /tmp/seed.sh"))
                with self.assertRaisesRegex(ValueError, "pinned global default"):
                    self.identity()

    def mounted(self, reference="registry.example/packages@sha256:" + "b" * 64, stage=None):
        stage = stage or "FROM ${PACKAGES} AS packages\n"
        recipe = self.recipe.replace("FROM archlinux:base@sha256:", stage + "FROM archlinux:base@sha256:", 1).replace(
            "RUN bash /tmp/install.sh", "RUN --mount=type=bind,from=packages,source=/bundle,target=/run/b,readonly bash /tmp/install.sh")
        return "ARG PACKAGES=" + reference + "\n" + recipe

    def test_mounted_image_digest_changes_identity(self):
        self.dockerfile.write_text(self.mounted())
        original = self.identity()
        self.assertEqual(CACHE.manifest(self.root)["images"], {"packages": "registry.example/packages@sha256:" + "b" * 64})
        self.dockerfile.write_text(self.mounted("registry.example/packages@sha256:" + "c" * 64))
        self.assertNotEqual(original, self.identity())

    def test_mounted_image_must_be_one_digest_pinned_stage(self):
        for name, recipe in {
                "tag only": self.mounted("registry.example/packages:latest"),
                "placeholder": self.mounted("registry.example/packages@sha256:" + "0" * 64),
                "unpinned argument": self.mounted().replace("ARG PACKAGES=", "ARG OTHER="),
                "extra instruction": self.mounted(stage="FROM ${PACKAGES} AS packages\nRUN true\n"),
                "unknown stage": self.mounted().replace("from=packages", "from=elsewhere")}.items():
            with self.subTest(name):
                self.dockerfile.write_text(recipe)
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
        self.assertIn("api/crates/layer4-runtime/image/build-omarchy-base.sh", value["inputs"])
        self.assertIn("api/crates/layer4-runtime/src/omarchy/build-hyprland.sh", value["inputs"])
        self.assertIn("api/crates/layer4-runtime/src/omarchy/hyprland-shm.patch", value["inputs"])
        self.assertNotIn("api/crates/layer4-runtime/src/omarchy/session.sh", value["inputs"])
        self.assertRegex(value["arguments"]["ARCH_SNAPSHOT"], r"^[0-9]{4}/[0-9]{2}/[0-9]{2}$")
        self.assertRegex(value["images"]["omarchy-packages"],
                         r"^ghcr\.io/chronoaiproject/cma-omarchy-packages@sha256:[0-9a-f]{64}$")


if __name__ == "__main__":
    unittest.main()
