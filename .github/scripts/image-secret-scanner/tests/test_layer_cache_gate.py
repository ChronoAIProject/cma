"""Exercise real publication-gate cold/warm scans without registry credentials."""
import json
import os
import sys
import tempfile
import unittest
from pathlib import Path
from unittest.mock import patch

from tests.fixtures import oci_archive, tar_bytes

sys.path.insert(0, str(Path(__file__).resolve().parents[2]))
import image_secret_gate as gate


class LayerCacheGateTests(unittest.TestCase):
    def test_real_gate_reuses_identical_layer_and_refuses_tampered_coverage(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            archive = root / "image.tar"
            oci_archive(archive, [tar_bytes([("app/file", b"ordinary public fixture\n")])])
            env = {"GITHUB_REPOSITORY": "ChronoAIProject/cma", "GITHUB_EVENT_NAME": "push",
                   "GITHUB_REF": "refs/heads/develop", "GITHUB_SHA": "a" * 40,
                   "GITHUB_RUN_ID": "22", "GITHUB_RUN_ATTEMPT": "1", "CD_COMPONENT": "web",
                   "RUNNER_TEMP": str(root), "CD_LAYER_SCAN_CACHE": "enabled"}
            original = gate.subprocess.run
            captured = []

            def launch(*args, **kwargs):
                captured.append(kwargs["env"])
                return original(*args, **kwargs)

            with patch.dict(os.environ, env), patch.object(gate.subprocess, "run", launch):
                cold = gate.scan_staging_archive(archive, root / "cold")
                warm = gate.scan_staging_archive(archive, root / "warm")
                self.assertEqual(cold, warm)
                report = json.loads((root / "warm/REPORT.json").read_bytes())
                self.assertEqual(report["detector_cache"]["hits"], 1)
                self.assertEqual(report["status"], "CLEAN")
                cache = root / "cma-layer-detector-output/cache.json"
                document = json.loads(cache.read_bytes())
                next(iter(document["layers"].values()))["coverage"].pop()
                cache.write_text(json.dumps(document))
                with self.assertRaises(gate.ScanRefusal):
                    gate.scan_staging_archive(archive, root / "refused")
            for child in captured:
                self.assertFalse({"GH_TOKEN", "GITHUB_TOKEN", "CD_NYXID_CLIENT_SECRET", "CD_LAYER_SCAN_CACHE"} & set(child))


if __name__ == "__main__":
    unittest.main()
