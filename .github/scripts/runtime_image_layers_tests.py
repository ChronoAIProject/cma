"""Layer evidence must reject churn, malformed archives and changed blob bytes."""

import copy
import hashlib
import io
import json
from pathlib import Path
import tarfile
import tempfile
import unittest
from unittest.mock import patch

from runtime_image_layers import compare, inventory, main


def fixture(path, layers=(b"base", b"dependencies", b"application"), *, platform="amd64", corrupt=False):
    entries = {}

    def blob(value):
        data = value if isinstance(value, bytes) else json.dumps(value).encode()
        digest = hashlib.sha256(data).hexdigest()
        entries["blobs/sha256/" + digest] = data
        return {"digest": "sha256:" + digest, "size": len(data)}

    descriptors = [blob(value) for value in layers]
    config = blob({"architecture": platform, "os": "linux",
                   "rootfs": {"diff_ids": [item["digest"] for item in descriptors]}})
    manifest = blob({"schemaVersion": 2, "config": config, "layers": descriptors})
    entries["index.json"] = json.dumps({"schemaVersion": 2, "manifests": [manifest]}).encode()
    if corrupt:
        entries["blobs/sha256/" + descriptors[0]["digest"][7:]] = b"oops"
    with tarfile.open(path, "w") as output:
        for name, data in entries.items():
            member = tarfile.TarInfo(name)
            member.size = len(data)
            output.addfile(member, io.BytesIO(data))


class LayerTests(unittest.TestCase):
    def test_real_archive_descriptors_measure_only_new_application_bytes(self):
        with tempfile.TemporaryDirectory() as directory:
            before, after = Path(directory) / "before.tar", Path(directory) / "after.tar"
            fixture(before)
            fixture(after, (b"base", b"dependencies", b"edited application"))
            result = compare(inventory(before), inventory(after), app_only=True)
            self.assertEqual(result["unchanged_prefix_layers"], 2)
            self.assertEqual(result["unchanged_prefix_bytes"], 16)
            self.assertEqual(result["new_compressed_bytes"], len(b"edited application"))
            self.assertEqual(result["new_layer_count"], 1)

    def test_repeated_packaging_has_no_new_bytes_and_cannot_fake_an_edit(self):
        with tempfile.TemporaryDirectory() as directory:
            path = Path(directory) / "image.tar"
            fixture(path)
            layers = inventory(path)
            self.assertEqual(compare(layers, layers)["new_compressed_bytes"], 0)
            with self.assertRaises(ValueError):
                compare(layers, layers, app_only=True)

    def test_cli_retains_only_verified_layer_evidence(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            before, after, output = root / "before.tar", root / "after.tar", root / "proof.json"
            fixture(before)
            fixture(after, (b"base", b"dependencies", b"edited"))
            with patch("sys.argv", ["layers", str(before), str(after), str(output), "--app-only"]):
                main()
            self.assertEqual(json.loads(output.read_text())["new_compressed_bytes"], 6)

    def test_dependency_churn_reorder_and_squash_fail_the_app_only_proof(self):
        with tempfile.TemporaryDirectory() as directory:
            path = Path(directory) / "image.tar"
            fixture(path)
            layers = inventory(path)
            for changed in (list(reversed(layers)), layers[:1], copy.deepcopy(layers)):
                changed[0] = dict(changed[0], digest="sha256:" + "f" * 64)
                with self.assertRaises(ValueError):
                    compare(layers, changed, app_only=True)
            with self.assertRaises(ValueError):
                compare([], layers)

    def test_corrupt_blob_wrong_platform_and_duplicate_member_are_refused(self):
        with tempfile.TemporaryDirectory() as directory:
            path = Path(directory) / "image.tar"
            for args in ({"corrupt": True}, {"platform": "arm64"}):
                fixture(path, **args)
                with self.assertRaises(ValueError):
                    inventory(path)
            fixture(path)
            with tarfile.open(path, "a") as output:
                member = tarfile.TarInfo("index.json")
                member.size = 2
                output.addfile(member, io.BytesIO(b"{}"))
            with self.assertRaises(ValueError):
                inventory(path)


if __name__ == "__main__":
    unittest.main()
