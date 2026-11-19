"""Cached binaries and wheels must match repository-pinned bytes before use."""
import hashlib
import tempfile
import unittest
from pathlib import Path
from unittest.mock import Mock, patch

from staging_cd_cache import cached, warm, wheels
from staging_cd_common import Refusal
import staging_cd_tools as tools


class CacheTests(unittest.TestCase):
    def test_valid_hit_and_cold_download_require_the_same_checksum(self):
        data = b"synthetic public tool"
        digest = hashlib.sha256(data).hexdigest()
        with tempfile.TemporaryDirectory() as directory:
            env = {"RUNNER_TEMP": directory}
            fetch = Mock(return_value=data)
            self.assertEqual(cached(env, "https://example.invalid/tool", digest, fetch), data)
            self.assertEqual(cached(env, "https://example.invalid/tool", digest, Mock(side_effect=AssertionError)), data)
            fetch.assert_called_once()
            path = Path(directory) / "cma-public-tool-cache" / digest
            path.write_bytes(b"corrupted")
            with self.assertRaises(Refusal):
                cached(env, "https://example.invalid/tool", digest, fetch)
            path.unlink()
            with self.assertRaises(Refusal):
                cached(env, "https://example.invalid/tool", digest, Mock(return_value=b"wrong"))
            path.symlink_to(Path(directory))
            with self.assertRaises(Refusal):
                cached(env, "https://example.invalid/tool", digest, fetch)

    def test_bundle_warms_only_pinned_public_bytes_and_rejects_unexpected_entries(self):
        data = b"fixture"
        digest = hashlib.sha256(data).hexdigest()
        with tempfile.TemporaryDirectory() as directory, patch("staging_cd_cache.wheels", return_value=[]):
            env = {"RUNNER_TEMP": directory}
            self.assertTrue(warm(env, [("https://example.invalid/tool", digest)], Mock(return_value=data)))
            (Path(directory) / "cma-public-tool-cache/unexpected").write_text("not a tool")
            with self.assertRaises(Refusal):
                warm(env, [("https://example.invalid/tool", digest)], Mock())

    def test_full_python_closure_is_exactly_pinned_and_contains_only_wheels(self):
        rows = wheels()
        self.assertEqual({row["name"] for row in rows},
                         {"PyYAML", "jsonschema", "attrs", "jsonschema-specifications", "referencing", "rpds-py", "typing_extensions"})
        for row in rows:
            self.assertRegex(row["sha256"], r"^[0-9a-f]{64}$")
            self.assertTrue(row["filename"].endswith(".whl"))
            self.assertTrue(row["url"].startswith("https://files.pythonhosted.org/"))

    def test_setup_watchdog_uses_remaining_admission_and_never_passes_credentials(self):
        from staging_cd_clock import timestamp
        import contextlib
        import io
        import os
        import subprocess
        env = {"CD_STARTED_AT": timestamp(1000), "PATH": "/bin", "RUNNER_TEMP": "/tmp",
               "NYXID_CHRONO_CD_KEY": "fixture", "GH_TOKEN": "fixture"}
        with patch.dict(os.environ, env, clear=True), patch.object(tools.sys, "argv", ["tools", "install"]), \
                patch("staging_cd_clock.time.time", return_value=1020), \
                patch.object(tools, "run_process", return_value=0) as run, \
                contextlib.redirect_stdout(io.StringIO()):
            self.assertEqual(tools.main(), 0)
            self.assertEqual(run.call_args.args[2], 100)
            self.assertNotIn("NYXID_CHRONO_CD_KEY", run.call_args.args[1])
            self.assertNotIn("GH_TOKEN", run.call_args.args[1])
            run.side_effect = subprocess.TimeoutExpired("fixture", 100)
            self.assertEqual(tools.main(), 1)


if __name__ == "__main__":
    unittest.main()
