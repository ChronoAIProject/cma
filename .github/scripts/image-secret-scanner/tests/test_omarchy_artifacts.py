"""A reviewed byte tuple cannot authorize a copied, altered or unproved finding."""

import copy
import io
import tempfile
import unittest
import zipfile
from pathlib import Path
from unittest.mock import patch

from image_secrets.common import ScanError
from image_secrets.key_vectors import sha
from image_secrets.omarchy_policy import finding_matches, identity, reviewed_omarchy_artifacts
from image_secrets.omarchy_proofs import prove
from image_secrets.omarchy_state import OmarchyState

PATH = "usr/lib/example/module.py"
DATA = b"runtime_password = runtime.member_reference\n"


def fixture(data=DATA):
    status = "var/lib/pacman/local/example-1.2-1/desc"
    listing = "var/lib/pacman/local/example-1.2-1/files"
    state = {
        status: b"%NAME%\nexample\n\n%VERSION%\n1.2-1\n\n%ARCH%\nx86_64\n\n",
        listing: ("%FILES%\n" + PATH + "\n\n").encode(),
    }
    entry = {
        "audit_id": "a" * 64,
        "component": "runtime-omarchy",
        "architecture": "amd64",
        "path": PATH,
        "member": None,
        "outer": {"path": PATH, "type": "file", "sha256": sha(data), "bytes": len(data)},
        "payload": {"sha256": sha(data), "bytes": len(data)},
        "match": {
            "rule": "gitleaks.image-secret-assignment",
            "line": 1,
            "file_sha256": sha(data),
            "candidate_sha256": sha(b"runtime.member_reference"),
            "line_sha256": sha(data),
        },
        "provenance": [
            {
                "kind": "pacman",
                "name": "example",
                "version": "1.2-1",
                "architecture": "x86_64",
                "status": status,
                "listing": listing,
                "owned": PATH,
                "status_sha256": sha(state[status]),
                "listing_sha256": sha(state[listing]),
            }
        ],
        "certificate": None,
        "marker": None,
        "classification": "public-material",
    }
    finding = {
        "scope": "layer",
        "image": 0,
        "layer": 0,
        "entry": 4,
        "path": "path-sha256:" + sha(PATH.encode()),
        **{k: v for k, v in entry["match"].items() if k != "line_sha256"},
    }
    return entry, state, finding


class OmarchyArtifactTests(unittest.TestCase):
    def test_exact_tuple_requires_version_ownership_and_provenance(self):
        entry, state, finding = fixture()
        self.assertTrue(prove(DATA, b"", state, {}, entry, {}))
        self.assertTrue(finding_matches(finding, entry))
        for changed in ("version", "architecture", "name", "missing-status", "ownership"):
            with self.subTest(changed=changed):
                altered = dict(state)
                desc, listing = entry["provenance"][0]["status"], entry["provenance"][0]["listing"]
                if changed == "missing-status":
                    del altered[desc]
                elif changed == "ownership":
                    altered[listing] = b"%FILES%\nelsewhere\n\n"
                else:
                    old = {"version": b"1.2-1", "architecture": b"x86_64", "name": b"example"}[changed]
                    altered[desc] = altered[desc].replace(old, b"different")
                self.assertFalse(prove(DATA, b"", altered, {}, entry, {}))

    def test_changed_body_line_candidate_and_copied_path_refuse(self):
        entry, state, finding = fixture()
        for addition in (
            b"extra",
            b"\x00additional material",
            b"\\u002dencoded material",
            bytes.fromhex("30820100020100"),
            b"(private-" + b"key (rsa (d #01#)))",
        ):
            self.assertFalse(prove(DATA + addition, b"", state, {}, entry, {}))
        for field, value in (
            ("path", "path-sha256:" + "0" * 64),
            ("candidate_sha256", "0" * 64),
            ("line", 2),
            ("file_sha256", "0" * 64),
            ("member", "0"),
        ):
            self.assertFalse(finding_matches(dict(finding, **{field: value}), entry))
        altered = copy.deepcopy(entry)
        altered["match"]["line_sha256"] = "0" * 64
        self.assertFalse(prove(DATA, b"", state, {}, altered, {}))

    def test_component_and_platform_have_no_inherited_entries(self):
        entry, _state, _finding = fixture()
        with patch("image_secrets.omarchy_state.policy", return_value=[entry]):
            self.assertEqual(OmarchyState("runtime", {"architecture": "amd64"}).entries, [])
            self.assertEqual(OmarchyState("runtime-omarchy", {"architecture": "arm64"}).entries, [])

    def test_nested_archive_binds_outer_member_index_and_contents(self):
        entry, state, finding = fixture()
        outer = io.BytesIO()
        with zipfile.ZipFile(outer, "w") as archive:
            archive.writestr("module.py", DATA)
        data = outer.getvalue()
        entry["outer"].update(sha256=sha(data), bytes=len(data))
        entry.update(path=PATH + "!module.py", member="0")
        self.assertTrue(prove(data, b"", state, {}, entry, {}))
        for field, value in (("path", PATH + "!copied.py"), ("member", "1"), ("member", "0/0")):
            changed = copy.deepcopy(entry)
            changed[field] = value
            self.assertFalse(prove(data, b"", state, {}, changed, {}))
        changed_data = io.BytesIO()
        with zipfile.ZipFile(changed_data, "w") as archive:
            archive.writestr("module.py", DATA + b"additional key material")
        self.assertFalse(prove(changed_data.getvalue(), b"", state, {}, entry, {}))
        self.assertFalse(finding_matches(finding, entry))

    def test_empty_bundle_requires_exact_file_and_never_parse_failure(self):
        entry, state, _finding = fixture(b"")
        entry["match"] = {"rule": "path.key-or-certificate"}
        entry["certificate"] = {"empty": True, "der_sha256": [], "pem_sha256": []}
        self.assertTrue(prove(b"", b"", state, {}, entry, {}))
        self.assertFalse(prove(b"truncated", b"", state, {}, entry, {}))
        entry["certificate"]["empty"] = False
        self.assertFalse(prove(b"", b"", state, {}, entry, {}))

    def test_symlink_substitution_and_target_change_refuse(self):
        entry, state, _finding = fixture(b"")
        target = "etc/ca/example.pem"
        entry["outer"] = {
            "path": PATH,
            "type": "symlink",
            "links": [{"path": PATH, "sha256": sha(b"target")}],
            "target": target,
            "target_sha256": sha(b""),
            "target_bytes": 0,
        }
        entry["certificate"] = {"empty": True, "der_sha256": [], "pem_sha256": []}
        state[target] = b""
        self.assertTrue(prove(b"", b"", state, {PATH: "target"}, entry, {}))
        self.assertFalse(prove(b"", b"", state, {PATH: "substituted"}, entry, {}))
        self.assertFalse(prove(b"", b"", dict(state, **{target: b"different"}), {PATH: "target"}, entry, {}))

    def test_receipt_cannot_transfer_component_or_hide_other_findings(self):
        entry, _state, finding = fixture()
        finding["finding_id"] = "b" * 64
        with tempfile.TemporaryDirectory() as directory:
            registry = Path(directory) / "policy.json"
            registry.write_bytes(b"synthetic reviewed policy")
            digest = sha(registry.read_bytes())
            result = {
                "policy": {"reviewed_omarchy_artifacts_sha256": digest},
                "findings": [finding],
                "reviewed_omarchy_artifacts": [
                    {
                        "finding_id": finding["finding_id"],
                        "entry_sha256": identity(entry),
                        "policy_sha256": digest,
                    }
                ],
            }
            with (
                patch("image_secrets.omarchy_policy.REGISTRY", registry),
                patch("image_secrets.omarchy_policy.policy", return_value=[entry]),
            ):
                self.assertEqual(
                    reviewed_omarchy_artifacts(result, "runtime-omarchy"),
                    {finding["finding_id"]: "public-material"},
                )
                with self.assertRaises(ScanError):
                    reviewed_omarchy_artifacts(result, "runtime")
                changed = copy.deepcopy(result)
                changed["findings"][0]["candidate_sha256"] = "0" * 64
                with self.assertRaises(ScanError):
                    reviewed_omarchy_artifacts(changed, "runtime-omarchy")


if __name__ == "__main__":
    unittest.main()
