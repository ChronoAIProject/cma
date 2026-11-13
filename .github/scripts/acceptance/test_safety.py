import datetime
import json
import tempfile
import unittest
from pathlib import Path
from unittest.mock import patch

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
            with patch("safety.SECRET_ROOT", Path(directory).resolve() / "private"):
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

    def test_cleanup_budget_and_time_are_reserved(self):
        http = Http()
        now = datetime.datetime(2026, 11, 11, 17, 30, tzinfo=datetime.timezone.utc)
        with self.assertRaises(Refusal):
            http.reserve_cleanup_time(now)
        with http.cleanup_budget():
            http.reserve_cleanup_time(now)
        http.count = 350
        with patch("safety.outside_quiet_window"), patch.object(http, "reserve_cleanup_time"):
            with self.assertRaisesRegex(Refusal, "request_budget_exhausted"):
                http.request("https://cma.nyxid.ai", "/auth/session")
            with http.cleanup_budget(), patch.object(http.opener, "open", side_effect=TimeoutError):
                with self.assertRaisesRegex(Refusal, "request_transport_failure"):
                    http.request("https://cma.nyxid.ai", "/auth/session")

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
