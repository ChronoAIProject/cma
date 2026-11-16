#!/usr/bin/env python3
import json
import os
import tempfile
import unittest
from pathlib import Path
from unittest.mock import patch

import migration_browser_fixture as fixture


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


if __name__ == "__main__":
    unittest.main()
