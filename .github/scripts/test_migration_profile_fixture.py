#!/usr/bin/env python3
import json
import os
import tempfile
import unittest
from pathlib import Path
from unittest.mock import patch

import migration_profile_fixture as fixture


class MigrationProfileFixtureTests(unittest.TestCase):
    def test_session_cookie_is_reused_and_subject_is_checked(self):
        with tempfile.TemporaryDirectory() as directory:
            state = Path(directory) / "auth.private"
            state.write_text(json.dumps({"cma_session": "ephemeral"}))
            responses = [(200, {}, json.dumps({
                "authenticated": True, "subject": "native-am-fixture-owner"
            }).encode())]
            with patch.dict(os.environ, {
                "MATRIX_IDENTITY_URL": "http://127.0.0.1:28080",
                "MATRIX_AUTH_STATE_FILE": str(state),
            }, clear=False), patch.object(fixture.Fixture, "_raw", side_effect=responses) as raw:
                value = fixture.Fixture("http://127.0.0.1:28081", Path(directory))
            raw.assert_called_once()
            self.assertEqual(value.cookies["cma_session"], "ephemeral")

    def test_session_identity_change_fails_closed(self):
        with tempfile.TemporaryDirectory() as directory:
            state = Path(directory) / "auth.private"
            state.write_text(json.dumps({"cma_session": "ephemeral"}))
            response = (200, {}, json.dumps({
                "authenticated": True, "subject": "unexpected-subject"
            }).encode())
            with patch.dict(os.environ, {
                "MATRIX_IDENTITY_URL": "http://127.0.0.1:28080",
                "MATRIX_AUTH_STATE_FILE": str(state),
            }, clear=False), patch.object(fixture.Fixture, "_raw", return_value=response):
                with self.assertRaisesRegex(RuntimeError, "identity_changed"):
                    fixture.Fixture("http://127.0.0.1:28081", Path(directory))


if __name__ == "__main__":
    unittest.main()
