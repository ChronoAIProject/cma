import hashlib
import json
from pathlib import Path
import tempfile
import unittest
from unittest.mock import Mock

from staging_cd_common import Refusal
from staging_cd_desktop_fixture import inspect_archive, publish_archive, require_staging_source
from staging_cd_registry import Registry, STAGING_DESKTOP_TAG_PREFIX


SOURCE = "a" * 40
DIGEST = "sha256:" + "b" * 64
CONFIG = "sha256:" + "c" * 64
STAGING = {
    "GITHUB_REPOSITORY": "ChronoAIProject/cma",
    "GITHUB_EVENT_NAME": "workflow_dispatch",
    "GITHUB_REF": "refs/heads/develop",
    "RUNNER_OS": "Linux",
    "RUNNER_ARCH": "X64",
}


class DesktopFixtureTests(unittest.TestCase):
    def test_only_native_dispatch_from_develop_can_publish(self):
        runner = Mock(return_value=(SOURCE + "\n").encode())
        require_staging_source(SOURCE, STAGING, runner)
        for key, value in (("GITHUB_REF", "refs/heads/main"),
                           ("GITHUB_EVENT_NAME", "push"), ("RUNNER_ARCH", "ARM64")):
            with self.subTest(key=key), self.assertRaises(Refusal):
                require_staging_source(SOURCE, dict(STAGING, **{key: value}), runner)
        with self.assertRaises(Refusal):
            require_staging_source("d" * 40, STAGING, runner)

    def test_desktop_supervision_and_lane_bind_to_config_bytes(self):
        labels = {
            "org.opencontainers.image.source": "https://github.com/ChronoAIProject/cma",
            "org.opencontainers.image.revision": SOURCE,
            "dev.chrono.cma.runtime-desktop": "1",
        }

        def inspect(variant, env):
            config = json.dumps({"os": "linux", "architecture": "amd64",
                                 "config": {"User": "10001:10001", "Labels": labels, "Env": env}}).encode()
            digest = "sha256:" + hashlib.sha256(config).hexdigest()
            manifest = json.dumps({"schemaVersion": 2, "config": {"digest": digest}, "layers": []}).encode()
            runner = Mock(side_effect=[manifest, config])
            result = inspect_archive(Path("fixture.oci.tar"), variant, SOURCE, runner)
            self.assertIn("--raw", runner.call_args.args[0])
            return result, digest

        enabled = ["CHRONO_RUNTIME_DESKTOP_SUPERVISION_ENABLED=true"]
        result, digest = inspect("debian", enabled)
        self.assertEqual(result[1], digest)
        with self.assertRaises(Refusal):
            inspect("debian", enabled * 2)
        with self.assertRaises(Refusal):
            inspect("omarchy", enabled)

    def test_existing_exact_tag_can_settle_without_a_registry_write(self):
        registry = Mock()
        registry.manifest.return_value = (DIGEST, {"config": {"digest": CONFIG}})
        runner = Mock()
        with tempfile.TemporaryDirectory() as directory:
            work = Path(directory)
            archive = work / "fixture.tar"
            archive.write_bytes(b"synthetic archive")
            digest = hashlib.sha256(archive.read_bytes()).hexdigest()
            image, tag = publish_archive(archive, "runtime", DIGEST, CONFIG, digest, work,
                                         "synthetic", "synthetic", registry, runner)
            self.assertEqual(image, "ghcr.io/chronoaiproject/cma-runtime@" + DIGEST)
            self.assertEqual(tag, STAGING_DESKTOP_TAG_PREFIX + DIGEST[7:])
            runner.assert_not_called()
            with self.assertRaises(Refusal):
                publish_archive(archive, "runtime", DIGEST, CONFIG, "0" * 64, work,
                                "synthetic", "synthetic", registry, runner)

    def test_staging_desktop_tags_are_reserved_for_runtime_packages(self):
        registry = Registry()
        registry._manifest = Mock()
        tag = STAGING_DESKTOP_TAG_PREFIX + DIGEST[7:]
        registry.manifest("ghcr.io/chronoaiproject/cma-runtime", tag)
        registry.manifest("ghcr.io/chronoaiproject/cma-runtime-omarchy", tag)
        with self.assertRaises(Refusal):
            registry.manifest("ghcr.io/chronoaiproject/cma-web", tag)
        self.assertEqual(registry._manifest.call_count, 2)


if __name__ == "__main__":
    unittest.main()
