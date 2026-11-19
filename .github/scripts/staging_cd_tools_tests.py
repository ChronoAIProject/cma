"""Pinned tool setup fails closed and keeps ambient credentials out of children."""
import contextlib
import hashlib
import io
import os
import tarfile
import tempfile
import unittest
from pathlib import Path
from unittest.mock import Mock, patch

from staging_cd_common import Refusal
from staging_cd_runtime import run_private
import staging_cd_tools as tools


class ToolTests(unittest.TestCase):
    def archive(self, link=False):
        stream = io.BytesIO()
        with tarfile.open(fileobj=stream, mode="w:gz") as archive:
            member = tarfile.TarInfo(tools.NYXID_MEMBER)
            member.type = tarfile.SYMTYPE if link else tarfile.REGTYPE
            member.linkname = "/outside" if link else ""
            member.size = 0 if link else 7
            archive.addfile(member, None if link else io.BytesIO(b"fixture"))
        return stream.getvalue()

    def test_install_pins_downloads_and_tools_are_available_in_a_clean_home(self):
        with tempfile.TemporaryDirectory() as directory:
            env = {"RUNNER_TEMP": directory, "PATH": os.environ["PATH"], "HOME": "/unrelated",
                   "GH_TOKEN": "fixture", "NYXID_CHRONO_CD_KEY": "fixture", "PIP_INDEX_URL": "https://example.invalid"}
            root = tools.tools_path(env)
            calls = []
            def run(args, **kwargs):
                calls.append((args, kwargs))
                if "venv" in args:
                    (root / "venv/bin").mkdir(parents=True)
                output = "--access-token-env --data --header v0.30.1"
                if args == ["gitleaks", "version"]:
                    output = "8.30.1"
                return Mock(returncode=0, stdout=output)
            fetch = Mock(side_effect=[self.archive(), b"fixture"])
            with patch.object(tools, "cached", return_value=b"synthetic wheel") as cache, \
                    patch.object(tools.sys, "platform", "linux"), \
                    patch.object(tools.os, "uname", return_value=Mock(machine="x86_64")):
                self.assertTrue(tools.install(env, fetch, run))
            self.assertEqual(cache.call_count, len(tools.wheels()))
            install = next(args for args, _ in calls if "install" in args)
            self.assertIn("--no-index", install)
            self.assertIn("--no-deps", install)
            self.assertEqual([Path(arg).name for arg in install if arg.endswith(".whl")],
                             [row["filename"] for row in tools.wheels()])
            self.assertTrue(any(args[-2:] == ["pip", "check"] for args, _ in calls))
            self.assertTrue(any(args[-1] == "import yaml, jsonschema" for args, _ in calls))
            self.assertEqual(fetch.call_args_list[0].args, (tools.NYXID_URL, tools.NYXID_SHA256))
            self.assertEqual(fetch.call_args_list[1].args, (tools.BUILDX_URL, tools.BUILDX_SHA256))
            for _, kwargs in calls:
                self.assertNotIn("GH_TOKEN", kwargs["env"])
                self.assertNotIn("NYXID_CHRONO_CD_KEY", kwargs["env"])
                self.assertNotIn("PIP_INDEX_URL", kwargs["env"])
                self.assertTrue(kwargs["capture_output"])
            self.assertEqual((root / "docker/config.json").read_text(), "{}\n")
            self.assertEqual(tools.environment(env)["HOME"], str(root / "home"))
            self.assertEqual(list((root / "home").iterdir()), [])
            self.assertTrue(tools.cleanup(env))
            self.assertFalse(root.exists())

    def test_download_must_match_the_pinned_checksum(self):
        response = Mock(status=200)
        response.read.return_value = b"fixture"
        with patch.object(tools.urllib.request, "urlopen", return_value=contextlib.nullcontext(response)):
            self.assertEqual(tools.download("https://example.invalid", hashlib.sha256(b"fixture").hexdigest()), b"fixture")
            with self.assertRaises(Refusal):
                tools.download("https://example.invalid", "0" * 64)

    def test_private_requirements_must_still_match_the_prepared_pins(self):
        with tempfile.TemporaryDirectory() as directory:
            env = {"RUNNER_TEMP": directory, "GITHUB_WORKSPACE": directory,
                   "GITHUB_RUN_ID": "123", "GITHUB_RUN_ATTEMPT": "1", "PATH": os.environ["PATH"],
                   "NYXID_CHRONO_CD_KEY": "fixture", "CD_INFRA_TOKEN": "fixture"}
            requirements = tools.checkout_path(env) / "staging/requirements.txt"
            requirements.parent.mkdir(parents=True)
            requirements.write_text("\n".join(tools.REQUIREMENTS))
            self.assertTrue(tools.verify_requirements(env))
            for text in ("PyYAML", "--index-url https://example.invalid", "PyYAML==6.0.2", ""):
                requirements.write_text(text)
                with self.assertRaises(Refusal):
                    tools.verify_requirements(env)
            requirements.unlink()
            requirements.symlink_to(Path(directory))
            with self.assertRaises(Refusal):
                tools.verify_requirements(env)

    def test_restore_uses_only_the_venv_verified_by_the_current_producer(self):
        with tempfile.TemporaryDirectory() as directory:
            env = {"RUNNER_TEMP": directory, "GITHUB_OUTPUT": str(Path(directory) / "outputs"),
                   "PATH": os.environ["PATH"]}
            root = tools.tools_path(env)
            def build(_env, root, _command):
                (root / "venv/bin").mkdir(parents=True)
                (root / "venv/bin/python").write_bytes(b"synthetic interpreter")
                (root / "home").mkdir()
                (root / "home/config").write_bytes(b"excluded")
            with patch.object(tools, "build_venv", side_effect=build):
                self.assertTrue(tools.prepare_venv_cache(env))
            tools.cleanup(env)
            archive = tools.cache_path(env) / "venv.tar"
            digest = hashlib.sha256(archive.read_bytes()).hexdigest()
            self.assertEqual(Path(env["GITHUB_OUTPUT"]).read_text(), f"sha256={digest}\n")
            self.assertFalse(tools.restore_venv(env, root))  # Manual calls have no producer.
            env["CD_VENV_SHA256"] = digest
            with patch.object(tools, "build_venv", side_effect=AssertionError), \
                    patch.object(tools.sys, "platform", "linux"), \
                    patch.object(tools.os, "uname", return_value=Mock(machine="x86_64")):
                runner = Mock(return_value=Mock(returncode=0, stdout="--access-token-env --data --header v0.30.1"))
                self.assertTrue(tools.install(env, Mock(side_effect=[self.archive(), b"fixture"]), runner))
            self.assertEqual((root / "venv/bin/python").read_bytes(), b"synthetic interpreter")
            self.assertEqual(list((root / "home").iterdir()), [])
            self.assertIn(["-m", "pip", "check"], [call.args[0][1:] for call in runner.call_args_list])
            tools.cleanup(env)
            archive.write_bytes(b"corrupted")
            for expected in (digest, "malformed"):
                env["CD_VENV_SHA256"] = expected
                with self.assertRaises(Refusal):
                    tools.restore_venv(env, root)
                self.assertFalse(root.exists())
            env["CD_VENV_SHA256"] = digest
            archive.unlink()
            self.assertFalse(tools.restore_venv(env, root))
            archive.symlink_to(Path(directory) / "missing")
            with self.assertRaises(Refusal):
                tools.restore_venv(env, root)

    def test_setup_errors_do_not_emit_captured_output(self):
        with tempfile.TemporaryDirectory() as directory:
            env = {"RUNNER_TEMP": directory, "PATH": os.environ["PATH"]}
            def failure(*args, **kwargs):
                print("private diagnostic")
                os.write(2, b"private native diagnostic")
                return Mock(returncode=1, stdout="private diagnostic")
            output = io.StringIO()
            with patch.object(tools.sys, "platform", "linux"), patch.object(tools.os, "uname", return_value=Mock(machine="x86_64")), contextlib.redirect_stdout(output):
                self.assertEqual(run_private(lambda _: tools.install(env, Mock(), failure), None), 1)
            self.assertEqual(output.getvalue(), "FAIL\n")

    def test_symlinked_tools_and_unsupported_runners_are_refused(self):
        with tempfile.TemporaryDirectory() as directory:
            env = {"RUNNER_TEMP": directory}
            root = Path(directory) / "cma-deploy-tools"
            root.symlink_to(Path(directory))
            with self.assertRaises(Refusal):
                tools.cleanup(env)
            with patch.object(tools.sys, "platform", "darwin"), self.assertRaises(Refusal):
                tools.install(env)
