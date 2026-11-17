#!/usr/bin/env python3
import json
import os
import tempfile
import unittest
from pathlib import Path
from unittest.mock import patch

import migration_browser_fixture as fixture
import migration_profile_fixture as profile_fixture


class MigrationProfileFixtureTests(unittest.TestCase):
    def test_session_cookie_is_reused_and_subject_is_checked(self):
        with tempfile.TemporaryDirectory() as directory:
            state = Path(directory) / "auth.private"
            state.write_text(json.dumps({"cma_session": "ephemeral"}))
            responses = [(200, {}, json.dumps({
                "authenticated": True, "subject": "native-am-fixture-owner"
            }).encode())]
            with patch.object(fixture.BrowserFixture, "raw", side_effect=responses) as raw:
                value = fixture.BrowserFixture("http://127.0.0.1:28081",
                                               "http://127.0.0.1:28080", state)
                value.cookies = {"cma_session": "ephemeral"}
                value.check_session()
            raw.assert_called_once()
            self.assertEqual(value.cookies["cma_session"], "ephemeral")

    def test_session_identity_change_fails_closed(self):
        with tempfile.TemporaryDirectory() as directory:
            state = Path(directory) / "auth.private"
            state.write_text(json.dumps({"cma_session": "ephemeral"}))
            response = (200, {}, json.dumps({
                "authenticated": True, "subject": "unexpected-subject"
            }).encode())
            with patch.object(fixture.BrowserFixture, "raw", return_value=response):
                value = fixture.BrowserFixture("http://127.0.0.1:28081",
                                               "http://127.0.0.1:28080", state)
                with self.assertRaisesRegex(RuntimeError, "subject_changed"):
                    value.check_session()

    def test_admin_route_requires_ephemeral_bearer_and_does_not_log_it(self):
        value = profile_fixture.Fixture.__new__(profile_fixture.Fixture)
        value.admin_bearer = "ephemeral-admin-token"
        with patch.object(value, "request", return_value={"revision": 4}) as request:
            self.assertEqual(value.admin_request("GET", "/api/v1/admin/settings"),
                             {"revision": 4})
        request.assert_called_once_with("GET", "/api/v1/admin/settings",
                                        body=None, status=200,
                                        bearer="ephemeral-admin-token",
                                        use_cookies=False)

        value.admin_bearer = None
        with self.assertRaisesRegex(RuntimeError, "admin fixture bearer"):
            value.admin_request("GET", "/api/v1/admin/settings")

    def test_admin_activity_follows_cursor_pages(self):
        value = profile_fixture.Fixture.__new__(profile_fixture.Fixture)
        value.admin_bearer = "ephemeral-admin-token"
        with patch.object(profile_fixture.time, "sleep"), patch.object(value, "admin_request", side_effect=[
                {"items": [{"id": "new"}], "next": "opaque cursor/+"},
                {"items": [{"id": "old"}], "next": None},
        ]) as request:
            self.assertEqual(value.admin_activity(), [{"id": "new"}, {"id": "old"}])
        request.assert_any_call("GET", "/api/v1/admin/activity?limit=50")
        request.assert_any_call("GET", "/api/v1/admin/activity?limit=50&cursor=opaque%20cursor%2F%2B")

    def test_admin_activity_rejects_malformed_pages_and_cursors(self):
        value = profile_fixture.Fixture.__new__(profile_fixture.Fixture)
        cases = [None, [], {}, {"items": []}, {"items": {}, "next": None},
                 {"items": [None], "next": None},
                 {"items": [{"id": ""}], "next": None},
                 {"items": [{"id": 1}], "next": None},
                 {"items": [{"id": str(i)} for i in range(51)], "next": None}]
        cases.extend({"items": [], "next": cursor}
                     for cursor in ("", False, 0, [], {}, "x" * 4097))
        for page in cases:
            with self.subTest(page=page), patch.object(value, "admin_request", return_value=page):
                with self.assertRaises(RuntimeError):
                    value.admin_activity()

    def test_admin_activity_rejects_repeated_rows_or_cursors(self):
        value = profile_fixture.Fixture.__new__(profile_fixture.Fixture)
        for pages in [
            [{"items": [{"id": "one"}], "next": "cursor"},
             {"items": [{"id": "one"}], "next": None}],
            [{"items": [], "next": "cursor"}, {"items": [], "next": "cursor"}],
        ]:
            with self.subTest(pages=pages), patch.object(profile_fixture.time, "sleep"), \
                    patch.object(value, "admin_request", side_effect=pages):
                with self.assertRaisesRegex(RuntimeError, "repeated"):
                    value.admin_activity()

    def test_admin_activity_has_a_fixed_page_bound(self):
        value = profile_fixture.Fixture.__new__(profile_fixture.Fixture)
        pages = [{"items": [], "next": str(i)} for i in range(20)]
        with patch.object(profile_fixture.time, "sleep") as pause, \
                patch.object(value, "admin_request", side_effect=pages) as request:
            with self.assertRaisesRegex(RuntimeError, "bounded pages"):
                value.admin_activity()
        self.assertEqual(request.call_count, 20)
        self.assertEqual(pause.call_count, 19)
        pause.assert_called_with(2)

    def test_admin_activity_propagates_http_failure(self):
        value = profile_fixture.Fixture.__new__(profile_fixture.Fixture)
        with patch.object(value, "admin_request", side_effect=RuntimeError("http failure")) as request:
            with self.assertRaisesRegex(RuntimeError, "http failure"):
                value.admin_activity()
        self.assertEqual(request.call_count, 1)


if __name__ == "__main__":
    unittest.main()
