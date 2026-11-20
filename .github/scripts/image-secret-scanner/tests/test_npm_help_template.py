"""Require complete signed placeholder examples at the publication boundary."""

import unittest

from image_secrets import npm_help_template
from image_secrets.common import digest_bytes
from image_secrets.gate import FORMAT_MARKER_ADVISORY
from tests import test_derived_payload as derived
from tests import test_package_policy as packages


class NpmHelpTemplateTests(unittest.TestCase):
    setUp = packages.PackageTests.setUp
    tearDown = packages.PackageTests.tearDown
    scan = packages.PackageTests.scan
    manifest = packages.PackageTests.manifest
    detector_scan = packages.PackageTests.detector_scan
    fixture = derived.DerivedPayloadTests.fixture
    publish = derived.DerivedPayloadTests.publish

    def example_fixture(self, name, *, scope="runtime", placeholder=b"XXXX", extra=b"",
                        ending=b"\n", normalize_digest=True):
        sample = npm_help_template.example(b"\\\\n").replace(b"XXXX", placeholder, 1)
        prefix = b"let usage = { description: `"
        template = ending.join((b"", b"Public help.", b"    " + sample, b"Use the caller's configuration.", b""))
        payload = prefix + template + b"` };\n" + extra
        source = ("package/" + npm_help_template.MEMBER, payload)
        archive, evidence, raw, _ = self.fixture(name, npm_help_template.PATH, source, payload, component=scope)
        [image] = raw["inventory"]["images"]
        evidence["packages"][0].update(name="npm", authority_type="pinned-upstream")
        [entry] = evidence["entries"]
        entry["path"] = npm_help_template.PATH
        [finding] = [f for f in raw["blocking_findings"] if f["rule"] == npm_help_template.RULE]
        line = b"    " + sample
        left = payload.index(line)
        begin = payload.index(npm_help_template.OPEN_LABEL)
        ctx = {"offset": len(prefix), "size": len(template), "sha256": digest_bytes(template)}
        cooked = template.replace(b"\\\\", b"\\")
        if normalize_digest:
            cooked = cooked.replace(ending, b"\n")
        entry["npm_help_template"] = {
            "raw_template": ctx, "cooked_template_sha256": digest_bytes(cooked),
            "raw_example": {"offset": left, "size": len(line), "sha256": digest_bytes(line)},
            "cooked_example_sha256": digest_bytes(npm_help_template.cook(line)),
        }
        span = {"offset": begin, "size": len(npm_help_template.OPEN_LABEL),
                "sha256": digest_bytes(npm_help_template.OPEN_LABEL), "context": ctx,
                "source": {"path": source[0], "line": 3, "sha256": digest_bytes(line)}}
        entry["reviewed_literals"] = {
            "archive_sha256": raw["archive_sha256"], "image_config": image["config_digest"],
            "image_manifest": image["manifest"], "findings": [{"rule": finding["rule"],
                                                               "finding_id": finding["finding_id"], "spans": [span]}],
        }
        entry["matches"] = [{"rule": finding["rule"], "candidate_sha256": None, "line": None,
                             "explanation": FORMAT_MARKER_ADVISORY, "proof_sha256": digest_bytes(line)}]
        return archive, evidence, raw

    def test_exact_raw_and_cooked_template_is_scoped_to_the_signed_package(self):
        for scope in ("runtime", "runtime-omarchy"):
            with self.subTest(scope=scope):
                archive, evidence, raw = self.example_fixture(scope, scope=scope)
                result = self.publish(archive, evidence, f"accepted-{scope}")
                self.assertEqual(result["findings"], raw["findings"])
                [receipt] = result["package_evidence"]["entries"]
                self.assertEqual(receipt["npm_help_template"], evidence["entries"][0]["npm_help_template"])

    def test_wrong_cooked_template_keeps_the_raw_finding(self):
        archive, evidence, raw = self.example_fixture("wrong-cooked")
        evidence["entries"][0]["npm_help_template"]["cooked_template_sha256"] = "f" * 64
        refused = self.publish(archive, evidence, "refused-cooked", blocks=True)
        self.assertEqual(refused["blocking_findings"], raw["blocking_findings"])

    def test_crlf_and_cr_have_lf_in_the_cooked_template(self):
        for name, ending in (("crlf", b"\r\n"), ("cr", b"\r")):
            with self.subTest(ending=name):
                archive, evidence, raw = self.example_fixture(name, ending=ending)
                accepted = self.publish(archive, evidence, f"accepted-{name}")
                self.assertEqual(accepted["findings"], raw["findings"])
                self.assertEqual(len(accepted["package_evidence"]["entries"]), 1)
                archive, evidence, raw = self.example_fixture(f"raw-{name}", ending=ending, normalize_digest=False)
                refused = self.publish(archive, evidence, f"refused-{name}", blocks=True)
                self.assertEqual(refused["blocking_findings"], raw["blocking_findings"])

    def test_an_unknown_body_or_extra_header_keeps_the_raw_findings(self):
        unexpected = b'const extra = "' + npm_help_template.OPEN_LABEL + b'";\n'
        for name, placeholder, extra in (("body", b"other", b""), ("extra", b"XXXX", unexpected)):
            with self.subTest(name=name):
                archive, evidence, raw = self.example_fixture(name, placeholder=placeholder, extra=extra)
                refused = self.publish(archive, evidence, f"refused-{name}", blocks=True)
                self.assertEqual(refused["blocking_findings"], raw["blocking_findings"])
                self.assertEqual(refused["package_evidence"]["entries"], [])
