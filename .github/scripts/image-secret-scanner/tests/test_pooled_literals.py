"""Bound a reviewed marker only by its exactly pinned read-only vector constant pool."""

import copy
import tarfile
import tempfile
import unittest
from pathlib import Path
from unittest.mock import patch

from image_secrets import pooled_literals, reviewed_literals
from image_secrets.common import ScanError, digest_bytes
from image_secrets.gate import FORMAT_MARKER_ADVISORY
from tests import fixtures
from tests import test_derived_payload as derived_tests
from tests import test_package_policy as packages
from tests.test_elf_nul_key_view import OFFSET, elf

RULE = "content.private-key-header"
NATIVE = b"-----BEGIN " + b"PRIVATE KEY-----"
POOLED = b"-----BEGIN ENCRYPTED " + b"PRIVATE KEY-----"
FOOTER = b"-----END ENCRYPTED " + b"PRIVATE KEY-----"


def pool():
    # Overlapping 16-byte slices, as a compiler emits them for inlined copies.
    return b"".join((POOLED[:16], POOLED[16:32], FOOTER[:16], FOOTER[16:32], FOOTER[-16:]))


class PooledLiteralTests(unittest.TestCase):
    def setUp(self):
        self.scratch = tempfile.TemporaryDirectory(prefix="pooled-literals-")
        self.root = Path(self.scratch.name)

    def tearDown(self):
        self.scratch.cleanup()

    def fixture(self, *, misalign=0, writable=False, native=True):
        prefix = b"\0" * 16 + (NATIVE + b"\0" if native else b"")
        prefix += b"\0" * (-(OFFSET + len(prefix)) % 16 + misalign)
        body = prefix + pool() + b"\0" * 16
        data = elf(body, writable=writable)
        path = self.root / "member"
        path.write_bytes(data)
        start = data.index(POOLED)
        context = {"offset": start, "size": len(pool()), "sha256": digest_bytes(data[start:start + len(pool())])}
        source = {"path": "src/fixture-labels.cpp", "line": 1, "sha256": "a" * 64}
        spans = [{"offset": start, "size": len(POOLED), "sha256": digest_bytes(POOLED), "context": context,
                  "source": source}]
        if native:
            # A NUL-terminated sibling keeps its native boundary check.
            at = data.index(NATIVE)
            spans.insert(0, {"offset": at, "size": len(NATIVE), "sha256": digest_bytes(NATIVE), "source": source,
                             "context": {"offset": at - 4, "size": len(NATIVE) + 8,
                                         "sha256": digest_bytes(data[at - 4:at + len(NATIVE) + 4])}})
        proof = {"findings": [{"rule": RULE, "spans": spans}]}
        pooled = [{"span": {key: spans[-1][key] for key in ("offset", "size", "sha256")}, "context": context}]
        return path, proof, pooled

    def test_exact_pool_bounds_only_the_pinned_marker(self):
        path, proof, pooled = self.fixture()
        self.assertEqual(reviewed_literals.verify(path, b"metadata", proof), {})
        self.assertEqual(set(reviewed_literals.verify(path, b"metadata", proof, pooled=pooled)), {RULE})

    def test_changed_pool_offset_alignment_or_mapping_refuses(self):
        path, proof, pooled = self.fixture()
        changed = copy.deepcopy(pooled)
        changed[0]["context"]["sha256"] = digest_bytes(b"other pool")
        self.assertEqual(reviewed_literals.verify(path, b"metadata", proof, pooled=changed), {})
        moved = copy.deepcopy(pooled)
        for key in ("span", "context"):
            moved[0][key]["offset"] += 16
        self.assertEqual(reviewed_literals.verify(path, b"metadata", proof, pooled=moved), {})
        data = bytearray(path.read_bytes())
        data[pooled[0]["context"]["offset"] + 70] ^= 1
        path.write_bytes(bytes(data))
        self.assertEqual(reviewed_literals.verify(path, b"metadata", proof, pooled=pooled), {})
        for name, options in (("misaligned", {"misalign": 1}), ("writable", {"writable": True})):
            with self.subTest(name=name):
                path, proof, pooled = self.fixture(**options)
                # Every pinned digest matches the fixture; only alignment or mapping differs.
                self.assertEqual(pooled[0]["context"]["offset"] % 16 != 0, name == "misaligned")
                self.assertEqual(reviewed_literals.verify(path, b"metadata", proof, pooled=pooled), {})

    def test_catalog_pin_binds_package_member_span_and_source(self):
        path, proof, pooled = self.fixture()
        row = {"path": "usr/lib/plugins/libfixture.so", "file_sha256": digest_bytes(path.read_bytes()),
               "platform": "linux/amd64", "member": None, "reviewed_literals": proof, "pooled_literals": pooled}
        package = {"name": "fixture", "version": "1.0-1", "architecture": "x86_64", "artifact_sha256": "b" * 64,
                   "authority_type": "arch-signature"}
        pin = {key: package[key] for key in ("name", "version", "architecture", "artifact_sha256")}
        pin.update(path=row["path"], file_sha256=row["file_sha256"], span=pooled[0]["span"],
                   context=pooled[0]["context"], source=proof["findings"][0]["spans"][-1]["source"])
        with patch.object(pooled_literals, "PIN", pin):
            pooled_literals.validate(row, package)
            other_span = copy.deepcopy(pooled)
            other_span[0]["span"]["offset"] += 1
            other_source = copy.deepcopy(proof)
            other_source["findings"][0]["spans"][-1]["source"] = dict(pin["source"], line=2)
            for name, (target, change) in {
                "version": ("package", {"version": "1.0-2"}), "package": ("package", {"name": "other"}),
                "path": ("row", {"path": row["path"] + ".other"}), "file": ("row", {"file_sha256": "c" * 64}),
                "span": ("row", {"pooled_literals": other_span}), "source": ("row", {"reviewed_literals": other_source}),
                "count": ("row", {"pooled_literals": pooled * 2}),
            }.items():
                with self.subTest(name=name):
                    changed = {"row": dict(row), "package": dict(package)}
                    changed[target].update(change)
                    with self.assertRaises(ScanError):
                        pooled_literals.validate(changed["row"], changed["package"])


class PooledLiteralGateTests(unittest.TestCase):
    setUp = packages.PackageTests.setUp
    tearDown = packages.PackageTests.tearDown
    scan = packages.PackageTests.scan
    detector_scan = packages.PackageTests.detector_scan
    manifest = packages.PackageTests.manifest
    publish = derived_tests.DerivedPayloadTests.publish

    def test_whole_gate_needs_the_pinned_pool_for_the_unterminated_marker(self):
        helper = PooledLiteralTests()
        helper.root = self.root
        member, proof, pooled = helper.fixture(native=False)
        contents = member.read_bytes()
        path = "usr/lib/plugins/libfixture.so"
        supplied = fixtures.tar_bytes([(path, contents)])
        layer = fixtures.tar_bytes([(path, contents), ("etc/machine-id", b""),
                                    ("var/lib/dbus/machine-id", (tarfile.SYMTYPE, "/etc/machine-id"))])
        archive = self.root / "pooled-literals.tar"
        fixtures.oci_archive(archive, [layer])
        raw = self.detector_scan(archive, "runtime-omarchy")
        [image] = raw["inventory"]["images"]
        [marker] = [row for row in raw["blocking_findings"] if row["rule"] == RULE]
        manifest = self.manifest(supplied, layer, component="runtime-omarchy", file=path)
        manifest["packages"][0].update(name="fixture", version="1.0-1", authority_type="arch-signature",
                                       architecture="x86_64")
        [row] = manifest["entries"]
        identity = {"archive_sha256": raw["archive_sha256"], "image_config": image["config_digest"],
                    "image_manifest": image["manifest"]}
        [finding] = proof["findings"]
        row["reviewed_literals"] = {**identity, "findings": [dict(finding, finding_id=marker["finding_id"])]}
        row["matches"] = [{"rule": RULE, "candidate_sha256": None, "line": None, "explanation": FORMAT_MARKER_ADVISORY,
                           "proof_sha256": "a" * 64}]
        package = manifest["packages"][0]
        pin = {key: package[key] for key in ("name", "version", "architecture", "artifact_sha256")}
        pin.update(path=path, file_sha256=row["file_sha256"], span=pooled[0]["span"], context=pooled[0]["context"],
                   source=finding["spans"][-1]["source"])
        with patch.object(pooled_literals, "PIN", pin):
            refused = self.publish(archive, manifest, "native-boundaries-only", blocks=True)
            self.assertEqual([item["rule"] for item in refused["blocking_findings"]], [RULE])
            row["pooled_literals"] = pooled
            clean = self.publish(archive, manifest, "pinned-pool")
            self.assertEqual(clean["findings"], raw["findings"])
            self.assertEqual(clean["blocking_findings"], [])


if __name__ == "__main__":
    unittest.main()
