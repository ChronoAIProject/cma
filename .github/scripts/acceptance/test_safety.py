import datetime
import json
import tempfile
import unittest
from pathlib import Path
from unittest.mock import patch

from probe import Probe
from safety import Http, Refusal, outside_quiet_window, read_private, write_private
from usage import REPORT_SUFFIXES


class SafetyTests(unittest.TestCase):
    def test_quiet_window_includes_entire_sgt_hour(self):
        for minute in (0, 30, 59):
            with self.assertRaises(Refusal):
                outside_quiet_window(datetime.datetime(2026, 11, 11, 18, minute, tzinfo=datetime.timezone.utc))
        outside_quiet_window(datetime.datetime(2026, 11, 11, 17, 59, tzinfo=datetime.timezone.utc))
        outside_quiet_window(datetime.datetime(2026, 11, 11, 19, 0, tzinfo=datetime.timezone.utc))

    def test_secret_permissions_and_symlinks_fail_closed(self):
        with tempfile.TemporaryDirectory() as directory:
            with patch("safety.SECRET_ROOT", Path(directory) / "private"):
                write_private("state.json", {"example": True})
                self.assertEqual(read_private("state.json"), {"example": True})
                path = Path(directory) / "private/state.json"
                self.assertEqual(path.stat().st_mode & 0o777, 0o600)
                path.chmod(0o644)
                with self.assertRaises(Refusal):
                    read_private("state.json")
                path.unlink()
                path.symlink_to(Path(directory) / "elsewhere")
                with self.assertRaises(Refusal):
                    write_private("state.json", {})

    def test_destination_never_follows_untrusted_host(self):
        http = Http()
        for origin, path in (("https://unapproved.invalid", "/"),
                             ("https://cma.nyxid.ai", "//unapproved.invalid/")):
            with self.assertRaises(Refusal):
                http.request(origin, path)
        self.assertEqual(http.count, 0)

    def context(self):
        p = object.__new__(Probe)
        p.accounts = {"a": {"subject": "test-a"}, "b": {"subject": "test-b"}}
        p.state = {"fixtures": {"document_a": {"id": "doc_owned"}}, "writes": {}}
        return p

    def test_mutations_are_restricted_to_test_scope(self):
        p = self.context()
        p.allow_write("a", "/api/v1/documents/doc_owned", "PATCH", {})
        p.allow_write("b", "/api/v1/documents/doc_owned/shares/test-a", "PUT", {})
        for path, method, body in (
            ("/api/v1/documents/doc_outside", "PATCH", {}),
            ("/api/v1/documents/doc_owned/shares/outside", "PUT", {}),
            ("/api/v1/admin/settings", "PATCH", {}),
            ("/api/v1/agents", "POST", {"sandbox_id": "outside"}),
            ("/api/v1/usage/metrics/reports", "POST", {"filters": {"owner": "outside"}}),
        ):
            with self.assertRaises(Refusal):
                p.allow_write("a", path, method, body)

    def test_uncertain_write_reuses_original_operation(self):
        p = self.context()
        p.save = lambda: None
        seen = []
        def request(*args, **kwargs):
            seen.append(kwargs["key"])
            if len(seen) == 1:
                raise Refusal("request_transport_failure")
            return 201, {}, b'{"id":"doc_owned"}', 1.0
        p.request = request
        with self.assertRaises(Refusal):
            p.write("create", "a", "/api/v1/documents", "POST", {"title": "synthetic"})
        p.write("create", "a", "/api/v1/documents", "POST", {"title": "synthetic"})
        self.assertEqual(seen[0], seen[1])
        p.write("create", "a", "/api/v1/documents", "POST", {"title": "synthetic"})
        self.assertEqual(len(seen), 2)
        with self.assertRaises(Refusal):
            p.write("create", "a", "/api/v1/documents", "POST", {"title": "changed"})

    def test_series_probes_include_required_bounded_query(self):
        self.assertIn("/series?metric=cpu&top=1", REPORT_SUFFIXES)
        self.assertNotIn("/series", REPORT_SUFFIXES)

    def test_stale_document_problem_matches_public_contract(self):
        root = Path(__file__).resolve().parents[3]
        fixture = root / "api/crates/layer1-public/tests/fixtures/public-contract/documents.json"
        cases = json.loads(fixture.read_text())["cases"]
        conflicts = [c for c in cases if c["request"]["method"] == "PATCH"
                     and c["response"]["status"] == 409]
        self.assertTrue(conflicts)
        self.assertIn("document_revision_conflict", {c["response"]["body"]["code"] for c in conflicts})


if __name__ == "__main__":
    unittest.main()
