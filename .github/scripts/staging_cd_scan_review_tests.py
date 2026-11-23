"""A release starts building only when the pinned controller accepts its scanner bundle."""
import base64
import contextlib
import io
import unittest

import staging_cd_scan_review as review
from image_scanner_policy import MANIFEST_SHA256
from staging_cd_common import Refusal

REVISION = "a" * 40
OTHER = "b" * 64


def policy(*pairs, extra=""):
    rows = "".join(f'        ("{version}", "{bundle}"),\n' for version, bundle in pairs)
    return ('"""Reviewed scanner version and manifest pairs."""\n\nfrom boundary import require\n\n'
            f"REVIEWED_SCANS = frozenset(\n    {{\n{rows}    }}\n)\n{extra}\n"
            "def reviewed_scan_policy(scan, reason):\n    require(isinstance(scan, dict), reason)\n")


class ScanReviewTests(unittest.TestCase):
    def reader(self, source=None, error=None, **fields):
        calls = []

        def factory(token):
            def read(path):
                calls.append((token, path))
                if error is not None:
                    raise error
                value = {"type": "file", "encoding": "base64",
                         "content": base64.b64encode(source.encode()).decode()}
                value.update(fields)
                return value
            return read
        return factory, calls

    def run_main(self, factory, revision=REVISION):
        stdout = io.StringIO()
        env = {"CD_INFRA_TOKEN": "synthetic-not-a-credential", "CD_INFRA_REVISION": revision}
        with contextlib.redirect_stdout(stdout):
            status = review.main(env, reader_factory=factory)
        return status, stdout.getvalue()

    def test_publication_pair_is_the_supported_version_of_the_pinned_bundle(self):
        version, bundle = review.publication_scan()
        self.assertEqual(bundle, MANIFEST_SHA256)
        self.assertRegex(version, r"^[0-9]+\.[0-9]+\.[0-9]+$")

    def test_accepted_bundle_passes_after_reading_the_exact_pinned_revision(self):
        factory, calls = self.reader(policy(("1.0.3", OTHER), review.publication_scan()))
        status, output = self.run_main(factory)
        self.assertEqual(status, 0)
        self.assertEqual(calls, [("synthetic-not-a-credential",
                                  f"repos/ChronoAIProject/cma-infra/contents/staging/scripts/reviewed_scan.py?ref={REVISION}")])
        self.assertNotIn("synthetic-not-a-credential", output.replace("::add-mask::synthetic-not-a-credential", ""))

    def test_unreviewed_bundle_fails_with_a_catalogued_reason_before_any_build(self):
        version, _ = review.publication_scan()
        factory, _ = self.reader(policy((version, OTHER)))
        status, output = self.run_main(factory)
        self.assertEqual(status, 1)
        self.assertIn("FAIL scan-review reason=unreviewed cause=operator", output)
        self.assertIn("SCAN-REVIEW bundle=" + MANIFEST_SHA256, output)
        self.assertIn("REVIEWED_SCANS", output)
        # The same digest under another version is not the reviewed pair.
        factory, _ = self.reader(policy(("9.9.9", MANIFEST_SHA256)))
        self.assertIn("reason=unreviewed", self.run_main(factory)[1])

    def test_unreadable_or_non_literal_policy_fails_closed_without_private_text(self):
        good = review.publication_scan()
        sources = [
            policy(good, extra='REVIEWED_SCANS = frozenset({("1.0.4", "' + OTHER + '")})\n'),
            policy(good).replace("frozenset(", "set("),
            policy(good).replace(f'"{good[1]}"', "digest()"),
            policy(("1.0.4", "not-a-digest"), good),
            policy(),
            "def broken(:\n",
        ]
        for source in sources:
            with self.subTest(source=source[:40]):
                factory, _ = self.reader(source)
                status, output = self.run_main(factory)
                self.assertEqual(status, 1)
                self.assertIn("FAIL scan-review reason=unreadable", output)
        for factory, _ in (self.reader(error=Refusal("private-marker response")),
                           self.reader(error=OSError("private-marker transport")),
                           self.reader(policy(good), type="dir"),
                           self.reader(policy(good), content=None)):
            status, output = self.run_main(factory)
            self.assertEqual(status, 1)
            self.assertIn("reason=unreadable", output)
            self.assertNotIn("private-marker", output)
        factory, calls = self.reader(policy(good))
        self.assertIn("reason=unreadable", self.run_main(factory, revision="main")[1])
        self.assertEqual(calls, [])

    def test_the_policy_module_is_parsed_never_executed(self):
        good = review.publication_scan()
        source = policy(good, extra="raise SystemExit('executed')\n")
        self.assertIn(good, review.reviewed_scans(source))
        with self.assertRaises(Refusal):
            review.reviewed_scans(policy(good).replace("frozenset(\n    {", "frozenset(\n    {*").replace("    }\n)", "    } | {('1.0.4', '" + OTHER + "')}\n)"))


if __name__ == "__main__":
    unittest.main()
