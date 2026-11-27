"""Exercise deterministic Web asset overlays without Docker or registry writes."""

import gzip
from dataclasses import replace
import io
import json
from pathlib import Path
import tarfile
import tempfile
import unittest
from unittest.mock import patch

from image_scanner_policy import MANIFEST_SHA256
from image_secret_gate import checksum
from staging_cd_common import Refusal
from staging_cd_receipts import published_receipt
from staging_cd_receipts_tests import fixture_identity
from staging_cd_web_overlay import (
    BASE_LABEL, BUILD_INFO, CURRENT, FALLBACK, HTML, MANIFEST_LABEL, OCI_CONFIG, OCI_LAYER,
    OCI_MANIFEST, REPOSITORY, SOURCE, WebArchive, add_member, assemble, descriptor, encoded, hashed, qualify,
)


def fixture_archive(directory, source, version, *, stylesheet=b"unchanged", extra=None, history=None):
    key = source[0]
    assets = {"index-" + key + ".js": ("release-" + key).encode(), "common.css": stylesheet}
    html = ('<script type="module" src="/assets/index-' + key + '.js"></script>').encode()
    info = encoded({"local": False, "revision": source, "version": version})
    payloads = list((CURRENT + name, payload) for name, payload in assets.items())
    payloads.extend(((HTML, html), (BUILD_INFO, info)))
    raw = io.BytesIO()
    with tarfile.open(fileobj=raw, mode="w", format=tarfile.USTAR_FORMAT) as layer:
        for name, payload in payloads:
            add_member(layer, name, payload)
        if extra is not None:
            layer.addfile(extra)
    compressed = gzip.compress(raw.getvalue(), mtime=0)
    config = {
        "os": "linux", "architecture": "amd64",
        "rootfs": {"type": "layers", "diff_ids": ["sha256:" + hashed(raw.getvalue())]},
        "config": {"User": "101:101", "Entrypoint": ["/entrypoint.sh"], "Labels": {
            "org.opencontainers.image.title": "CMA Web", "org.opencontainers.image.source": SOURCE,
            "org.opencontainers.image.revision": source, "org.opencontainers.image.version": version,
        }},
    }
    if history is not None:
        config["history"] = history
    config_payload = encoded(config)
    manifest_payload = encoded({
        "schemaVersion": 2, "mediaType": OCI_MANIFEST, "config": descriptor(config_payload, OCI_CONFIG),
        "layers": [descriptor(compressed, OCI_LAYER)],
    })
    manifest = descriptor(manifest_payload, OCI_MANIFEST)
    index = encoded({"schemaVersion": 2, "manifests": [manifest]})
    path = directory / (key + ".oci.tar")
    with tarfile.open(path, "x", format=tarfile.USTAR_FORMAT) as archive:
        add_member(archive, "oci-layout", b'{"imageLayoutVersion":"1.0.0"}')
        add_member(archive, "index.json", index)
        for item in (config_payload, manifest_payload, compressed):
            add_member(archive, "blobs/sha256/" + hashed(item), item)
    return WebArchive(path, manifest["digest"], source, version), config, html


def unpack_overlay(path):
    with tarfile.open(path) as archive:
        [raw] = json.load(archive.extractfile("index.json"))["manifests"]
        manifest = json.load(archive.extractfile("blobs/sha256/" + raw["digest"][7:]))
        config = json.load(archive.extractfile("blobs/sha256/" + manifest["config"]["digest"][7:]))
        layer = archive.extractfile("blobs/sha256/" + manifest["layers"][-1]["digest"][7:]).read()
    entries = {}
    with tarfile.open(fileobj=io.BytesIO(layer), mode="r:gz") as contents:
        for entry in contents:
            entries[entry.name] = contents.extractfile(entry).read()
    return raw["digest"], config, entries


class WebOverlayTests(unittest.TestCase):
    def setUp(self):
        temp = tempfile.TemporaryDirectory()
        self.addCleanup(temp.cleanup)
        self.root = Path(temp.name)
        self.first, _, _ = fixture_archive(self.root, "a" * 40, "0.22.8")
        self.incumbent, self.old_config, self.old_html = fixture_archive(self.root, "b" * 40, "0.22.9")
        self.candidate, self.new_config, self.new_html = fixture_archive(self.root, "c" * 40, "0.22.10")

    def test_a_through_c_include_older_still_in_window_without_rewriting_html(self):
        result = assemble(self.incumbent, self.candidate, [self.first], self.root, max_files=16, max_bytes=512)
        self.assertEqual(result["asset_count"], 4)
        proof = result["asset_manifest_sha256"]
        for role, version, expected, raw in (("prepare", "0.22.9", self.incumbent, self.old_config),
                                             ("promote", "0.22.10", self.candidate, self.new_config)):
            digest, config, entries = unpack_overlay(result[role]["archive"])
            self.assertEqual(digest, result[role]["digest"])
            self.assertEqual(config["config"]["Labels"]["org.opencontainers.image.version"], version)
            self.assertEqual(config["config"]["Labels"].pop(BASE_LABEL), REPOSITORY + "@" + expected.digest)
            self.assertEqual(config["config"]["Labels"].pop(MANIFEST_LABEL), proof)
            self.assertEqual(config["rootfs"]["diff_ids"][:-1], raw["rootfs"]["diff_ids"])
            self.assertEqual(config["config"], raw["config"])
            self.assertNotIn("history", config)
            self.assertNotIn(HTML, entries)
            marker = entries.pop(FALLBACK + "cma-asset-manifest-" + proof + ".json")
            self.assertEqual(hashed(marker), proof)
            self.assertEqual(set(entries), {FALLBACK + name for name in
                                             ("index-a.js", "index-b.js", "index-c.js", "common.css")})
        self.assertEqual(result["sentinels"]["incumbent"]["path"], "/assets/index-b.js")
        self.assertEqual(result["sentinels"]["incumbent"]["html_sha256"], hashed(self.old_html))
        self.assertEqual(result["sentinels"]["candidate"]["html_sha256"], hashed(self.new_html))

    def test_same_inputs_produce_exactly_the_same_archives(self):
        scope = (self.incumbent, self.candidate, [self.first], self.root)
        first = assemble(*scope, max_files=16, max_bytes=512)
        second = assemble(*scope, max_files=16, max_bytes=512)
        for key in ("prepare", "promote"):
            self.assertEqual(first[key]["digest"], second[key]["digest"])
            self.assertEqual(first[key]["archive"].read_bytes(), second[key]["archive"].read_bytes())
            self.assertNotEqual(first[key]["archive"], second[key]["archive"])

    def test_oci_blob_directories_are_explicit_before_their_members(self):
        result = assemble(self.incumbent, self.candidate, [], self.root, max_files=16, max_bytes=512)
        for role in ("prepare", "promote"):
            with self.subTest(role=role), tarfile.open(result[role]["archive"]) as archive:
                members = archive.getmembers()
                blobs = [entry for entry in members if entry.name.startswith("blobs/sha256/")]
                self.assertTrue(blobs)
                for name in ("blobs", "blobs/sha256"):
                    directory = archive.getmember(name)
                    self.assertTrue(directory.isdir())
                    self.assertEqual(directory.mode, 0o755)
                    self.assertLess(members.index(directory), members.index(blobs[0]))
                self.assertLess(members.index(archive.getmember("blobs")),
                                members.index(archive.getmember("blobs/sha256")))

    def test_overlay_records_its_layer_in_existing_oci_history(self):
        history = [{"created_by": "COPY Web files"}, {"created_by": "LABEL version", "empty_layer": True}]
        incumbent, _, _ = fixture_archive(self.root, "d" * 40, "0.22.11", history=history)
        candidate, _, _ = fixture_archive(self.root, "e" * 40, "0.22.12", history=history)
        result = assemble(incumbent, candidate, [], self.root, max_files=16, max_bytes=512)
        for role in ("prepare", "promote"):
            with self.subTest(role=role):
                _, config, _ = unpack_overlay(result[role]["archive"])
                self.assertEqual(config["history"][:-1], history)
                nonempty = [entry for entry in config["history"] if not entry.get("empty_layer", False)]
                self.assertEqual(len(nonempty), len(config["rootfs"]["diff_ids"]))
                self.assertFalse(config["history"][-1].get("empty_layer", False))

    def test_conflicting_immutable_path_refuses_pair_before_packaging(self):
        corrupt, _, _ = fixture_archive(self.root, "d" * 40, "0.22.11", stylesheet=b"different")
        with self.assertRaisesRegex(Refusal, "immutable asset bytes"):
            assemble(self.incumbent, corrupt, [self.first], self.root, max_files=16, max_bytes=512)
        self.assertFalse(list(self.root.glob("web-asset-overlays-*")))

    def test_qualified_annotation_binds_each_archive_and_its_own_report(self):
        result = assemble(self.incumbent, self.candidate, [self.first], self.root, max_files=16, max_bytes=512)
        base = REPOSITORY + "@" + self.incumbent.digest
        incumbent = {"image": base, "base_image": base, "version": self.incumbent.version,
                     "source_sha": self.incumbent.source}
        identity = fixture_identity(source=self.candidate.source, previous=self.incumbent.version,
                                    version=self.candidate.version)
        delivered = {}

        def scan(archive, directory, *, component, source):
            self.assertEqual((component, source), ("web", self.root))
            digest = checksum(archive)
            data = encoded({"archive_sha256": digest,
                            "counts": {"detected": 0, "blocking": 0, "advisory": 0, "reviewed": 0, "allowlisted": 0}})
            directory.mkdir()
            (directory / "REPORT.json").write_bytes(data)
            delivered[archive] = hashed(data)
            return digest, ("1.0.4", MANIFEST_SHA256)

        with patch("staging_cd_web_overlay.verified_staging_scan", side_effect=scan) as scanner:
            evidence = qualify(result, incumbent, identity, self.candidate.digest, "d" * 64, self.root)
            self.assertEqual(scanner.call_count, 2)
        promote = result["promote"]
        receipt = published_receipt(identity, promote["digest"], promote["archive_sha256"],
                                    evidence.pop("counts"), **evidence)
        self.assertEqual(receipt["asset_proof"]["retention_preimage_sha256"], "d" * 64)
        for role in ("prepare", "promote"):
            self.assertEqual(receipt["asset_proof"]["overlays"][role]["credential_report_sha256"],
                             delivered[result[role]["archive"]])

    def test_archive_change_before_scan_cannot_become_qualified(self):
        result = assemble(self.incumbent, self.candidate, [], self.root, max_files=16, max_bytes=512)
        result["prepare"]["archive"].write_bytes(b"changed after assembling")

        def scan(archive, directory, **options):
            digest = checksum(archive)
            directory.mkdir()
            (directory / "REPORT.json").write_bytes(encoded({"archive_sha256": digest}))
            return digest, ("1.0.4", MANIFEST_SHA256)

        base = REPOSITORY + "@" + self.incumbent.digest
        incumbent = {"image": base, "base_image": base, "version": self.incumbent.version,
                     "source_sha": self.incumbent.source}
        with patch("staging_cd_web_overlay.verified_staging_scan", side_effect=scan):
            with self.assertRaisesRegex(Refusal, "differs from its scan report"):
                qualify(result, incumbent, {}, self.candidate.digest, "d" * 64, self.root)

    def test_incomplete_or_ambiguous_input_refused(self):
        self.assertRaises(Refusal, assemble, self.incumbent, self.candidate, [self.incumbent],
                          self.root, max_files=16, max_bytes=512)
        wrong = WebArchive(self.candidate.archive, self.first.digest, self.candidate.source, self.candidate.version)
        with self.assertRaisesRegex(Refusal, "immutable release"):
            assemble(self.incumbent, wrong, [], self.root, max_files=16, max_bytes=512)
        with self.assertRaisesRegex(Refusal, "approved budget"):
            assemble(self.incumbent, self.candidate, [self.first], self.root, max_files=3, max_bytes=512)
        with self.assertRaisesRegex(Refusal, "differs from its finalized catalog"):
            retained = replace(self.first, catalog_sha256="b" * 64)
            assemble(self.incumbent, self.candidate, [retained], self.root, max_files=16, max_bytes=512)

    def test_whiteout_refused_as_ambiguous_approved_overlay(self):
        extra = tarfile.TarInfo("usr/share/nginx/current/.wh.assets")
        wrong, _, _ = fixture_archive(self.root, "d" * 40, "0.22.11", extra=extra)
        with self.assertRaisesRegex(Refusal, "overlays an approved path"):
            assemble(self.incumbent, wrong, [], self.root, max_files=16, max_bytes=512)
