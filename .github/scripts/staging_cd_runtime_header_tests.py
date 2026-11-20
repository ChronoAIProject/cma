"""The raw detector finding needs an independent proof for every payload header."""
import copy
import io
import tarfile
import tempfile
import unittest
from pathlib import Path
from unittest.mock import patch

from staging_cd_content_policy_tests import digest, finding, layer_identities, tar_bytes

import image_runtime_headers as headers
from image_content_policy import decisions

# Markers alone exercise the detector without putting keys in the test inputs.
MARKER = b"-----BEGIN " + b"PRIVATE KEY-----"
STAGING = {"CD_COMPONENT": "runtime", "GITHUB_EVENT_NAME": "push", "GITHUB_REF": "refs/heads/develop",
           "GITHUB_WORKFLOW_REF": "ChronoAIProject/cma/.github/workflows/staging-cd.yml@refs/heads/develop"}


class RuntimeHeaderTests(unittest.TestCase):
    def setUp(self):
        self.payload = b"synthetic\x00"
        self.rows = []
        for n in range(12):
            vector = MARKER + bytes([n])
            self.rows.append({"component": "runtime", "scope": "layer", "path": headers.PATH,
                              "rule": "gitleaks.private-key", "classification": "public-material",
                              "header_offset": len(self.payload), "vector_bytes": len(vector),
                              "candidate_sha256": digest(vector)})
            self.payload += vector + b"\x00"
        self.digest = digest(self.payload)
        for row in self.rows:
            row["file_sha256"] = self.digest
        patched = patch.object(headers, "FILE_SHA256", self.digest)
        patched.start()
        self.addCleanup(patched.stop)
        self.finding = finding(headers.PATH, rule=headers.RULE)
        self.temp = tempfile.TemporaryDirectory()
        self.addCleanup(self.temp.cleanup)
        self.layer = Path(self.temp.name) / "layer.tar"

    def prove(self, layer, requested):
        self.layer.write_bytes(layer)
        proof = headers.staging_proof("runtime", STAGING, self.rows)
        identities = layer_identities(self.layer, {}, requested, headers=proof)
        findings = [f for group in requested.values() for f in group]
        return decisions({}, "runtime", findings, identities, headers=proof)

    def test_exact_entry_with_twelve_vectors_and_separate_credential(self):
        other = finding("etc/other", entry=1, rule=headers.RULE)
        layer = tar_bytes([(headers.PATH, self.payload), ("etc/other", MARKER)])
        matches = self.prove(layer, {0: [self.finding], 1: [other]})
        self.assertEqual([(m["finding_id"], m["classification"]) for m in matches],
                         [(self.finding["finding_id"], "public-material")])

    def test_no_other_workflow_or_component_can_use_proof(self):
        for key in STAGING:
            for value in (None, "other"):
                env = dict(STAGING)
                if value is None:
                    del env[key]
                else:
                    env[key] = value
                with self.subTest(key=key, value=value):
                    self.assertIsNone(headers.staging_proof("runtime", env, self.rows))
        for component in ("runtime-omarchy", "am", "eg", "web", None):
            self.assertIsNone(headers.staging_proof(component, dict(STAGING, CD_COMPONENT=component), self.rows))

    def test_catalog_requires_twelve_distinct_non_overlapping_public_vectors(self):
        for edit in ({"file_sha256": "a" * 64}, {"classification": "REAL"}, {"header_offset": False},
                     {"header_offset": -1}, {"header_offset": self.rows[1]["header_offset"] - 1},
                     {"vector_bytes": 0}, {"vector_bytes": headers.MAX_BYTES},
                     {"candidate_sha256": self.rows[1]["candidate_sha256"]}):
            rows = copy.deepcopy(self.rows)
            rows[0].update(edit)
            with self.subTest(edit=edit):
                self.assertIsNone(headers.staging_proof("runtime", STAGING, rows))
        for rows in (self.rows[:-1], self.rows + self.rows[:1]):
            self.assertIsNone(headers.staging_proof("runtime", STAGING, rows))

    def test_offsets_lengths_and_vector_bytes_are_independently_verified(self):
        # Isolate the offset/range checks from the whole-library digest guard.
        for payload in (self.payload + MARKER, b"x" + self.payload, self.payload.replace(MARKER, b"x", 1),
                        self.payload[:-2], self.payload[:-2] + b"x\x00"):
            proof = headers.staging_proof("runtime", STAGING, self.rows)
            proof.observe(payload, self.digest, [self.finding])
            self.assertFalse(proof.confirmed)
        proof = headers.staging_proof("runtime", STAGING, self.rows)
        proof.observe(self.payload, "a" * 64, [self.finding])
        proof.observe(self.payload, self.digest, [dict(self.finding, line=1)])
        self.assertFalse(proof.confirmed)
        self.assertFalse(proof.needs(headers.PATH, headers.MAX_BYTES + 1, [self.finding]))
        self.assertFalse(proof.needs(headers.PATH, len(self.payload), []))

    def test_catalog_conflict_cannot_fall_back_to_header_proof(self):
        proof = headers.staging_proof("runtime", STAGING, self.rows)
        proof.observe(self.payload, self.digest, [self.finding])
        identities = {self.finding["finding_id"]: self.digest}
        key = ("runtime", "layer", self.finding["path"], headers.RULE, self.digest)
        for entry, expected in (({"classification": "REAL"}, ["REAL"]),
                                ({"classification": "REAL", "candidate_sha256": "a" * 64}, [])):
            matches = decisions({key: [entry]}, "runtime", [self.finding], identities, headers=proof)
            self.assertEqual([row["classification"] for row in matches], expected)

    def test_metadata_links_wrong_paths_and_changed_duplicate_entries(self):
        with tarfile.open(self.layer, "w") as archive:
            member = tarfile.TarInfo(headers.PATH)
            member.size = len(self.payload)
            member.pax_headers = {"comment": MARKER.decode()}
            archive.addfile(member, io.BytesIO(self.payload))
        self.assertEqual(self.prove(self.layer.read_bytes(), {0: [self.finding]}), [])
        for kind in (tarfile.SYMTYPE, tarfile.LNKTYPE):
            layer = tar_bytes([("target", self.payload), (headers.PATH, (kind, "/target"))])
            self.assertEqual(self.prove(layer, {1: [finding(headers.PATH, entry=1, rule=headers.RULE)]}), [])
        self.assertEqual(self.prove(tar_bytes([("other", self.payload)]),
                                    {0: [finding("other", rule=headers.RULE)]}), [])
        duplicate = finding(headers.PATH, entry=1, rule=headers.RULE)
        matches = self.prove(tar_bytes([(headers.PATH, self.payload), (headers.PATH, self.payload + MARKER)]),
                             {0: [self.finding], 1: [duplicate]})
        self.assertEqual([m["finding_id"] for m in matches], [self.finding["finding_id"]])


if __name__ == "__main__":
    unittest.main()
