"""Runtime inventory refusal precedes every registry or deployment mutation."""

import contextlib
import io
import os
from pathlib import Path
import tempfile
import unittest
from unittest.mock import Mock, patch

import image_secret_gate as SECRET
import release_image_publication as RELEASE
import runtime_image_hygiene as HYGIENE
import staging_cd_publish as STAGING
from runtime_image_hygiene_tests import docker_image, final_entries, layer


class RuntimeHygieneIntegrationTests(unittest.TestCase):
    def test_release_and_staging_refuse_private_storage_without_mutation(self):
        for component in ("runtime", "runtime-omarchy"):
            for boundary in ("release", "staging"):
                with self.subTest(component=component, boundary=boundary), tempfile.TemporaryDirectory() as directory:
                    root = Path(directory)
                    archive = root / "candidate.oci.tar"
                    docker_image(archive, [layer([("etc/pacman.d/gnupg/private-keys-v1.d/fixture",
                                                  b"synthetic non-secret test data", "file")]),
                                          layer(final_entries())])
                    writer, detector, smoke = Mock(), Mock(), Mock()
                    def scanner(path, output, **kwargs):
                        return SECRET.scan_archive(path, output, detector, component=component)
                    with patch.dict(os.environ, {"SKOPEO_OUTPUT_DIR": str(root),
                                                 "SKOPEO_COMMAND": "skopeo", "SKOPEO_AUTHFILE": "/auth"}), \
                            self.assertRaisesRegex(SECRET.ScanRefusal, "Runtime layer hygiene refused"):
                        if boundary == "release":
                            RELEASE.publish_archive(archive, component, "v1.2.3", "a" * 40,
                                                    "ghcr.io/chronoaiproject/cma-" + component,
                                                    scanner=scanner, run_command=writer)
                        else:
                            registry = Mock()
                            registry.manifest.return_value = None
                            with patch.object(STAGING, "inspect", return_value="sha256:" + "a" * 64):
                                identity = {"component": component, "version": "v1.2.3", "source": "a" * 40}
                                STAGING.publish(archive, identity, root,
                                                registry, "synthetic", "actor", scanner=scanner,
                                                runner=writer, smoke=smoke, reservation_check=lambda: True,
                                                source=Path(__file__).resolve().parents[2])
                    writer.assert_not_called()
                    detector.assert_not_called()
                    smoke.assert_not_called()

    def test_docker_gate_exports_inspected_immutable_identity(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            fixture = root / "fixture.tar"
            docker_image(fixture, [layer(final_entries())])
            identity = "sha256:" + "a" * 64
            def docker(command, **kwargs):
                if command[1:3] == ["image", "inspect"]:
                    return Mock(stdout=identity + "\n")
                self.assertEqual(command, ["docker", "image", "save", identity])
                kwargs["stdout"].write(fixture.read_bytes())
                return Mock(returncode=0)
            with patch.object(HYGIENE.subprocess, "run", side_effect=docker), \
                    patch.dict(os.environ, {"RUNNER_TEMP": str(root)}), \
                    contextlib.redirect_stdout(io.StringIO()) as output:
                self.assertEqual(HYGIENE.main(["--image", "runtime:test", "--component", "runtime",
                                               "--output", str(root / "report.json")]), 0)
            self.assertIn("empty_base_exceptions=0", output.getvalue())
            self.assertEqual(list(root.glob("runtime-image-inventory-*")), [])
            with patch.object(HYGIENE.subprocess, "run", return_value=Mock(stdout="untrusted")) as docker:
                with self.assertRaises(HYGIENE.HygieneRefusal):
                    HYGIENE.docker_archive("runtime:test", root / "export.tar")
                self.assertEqual(docker.call_count, 1)
                self.assertFalse((root / "export.tar").exists())

    def test_successful_layer_inventory_still_requires_complete_secret_scan(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            archive = root / "image.tar"
            docker_image(archive, [layer(final_entries())])
            detector = Mock(side_effect=SECRET.ScanRefusal("synthetic detector refusal"))
            with self.assertRaisesRegex(SECRET.ScanRefusal, "synthetic detector refusal"):
                SECRET.scan_archive(archive, root / "scan", detector, component="runtime")
            detector.assert_called_once()
            self.assertTrue((root / "scan-runtime-hygiene.json").is_file())


if __name__ == "__main__":
    unittest.main()
