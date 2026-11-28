"""Verify same-attempt staging admission of a separately tested native archive."""
import json
import subprocess
import tempfile
import unittest
import zipfile
from datetime import datetime, timezone
from pathlib import Path
from unittest.mock import Mock, patch

import staging_cd_native_archive as native
from staging_cd_common import Refusal
from staging_cd_receipts_tests import fixture_identity
from staging_cd_release_archive import ARCHIVE, MANIFEST, seal


class NativeArchiveTests(unittest.TestCase):
    def setUp(self):
        self.release = fixture_identity(component="am")
        self.image = ("sha256:" + "a" * 64, "sha256:" + "b" * 64,
                      self.release["build"], self.release["version"])
        now = datetime.now(timezone.utc).isoformat()
        self.run = {"repository": {"full_name": "ChronoAIProject/cma", "id": 1},
                    "head_repository": {"full_name": "ChronoAIProject/cma", "id": 1},
                    "id": self.release["run_id"], "run_attempt": self.release["run_attempt"],
                    "head_sha": self.release["source"], "head_branch": "develop",
                    "path": ".github/workflows/staging-cd.yml", "event": "push", "status": "in_progress"}
        self.job = {"name": "Release am / Native release image / Build and test", "status": "completed",
                    "conclusion": "success", "run_id": self.release["run_id"],
                    "run_attempt": self.release["run_attempt"], "head_sha": self.release["source"],
                    "started_at": now, "completed_at": now}
        self.artifact = {"id": 12, "name": native.artifact_name(self.release), "expired": False,
                         "size_in_bytes": 100, "digest": "sha256:" + "c" * 64, "created_at": now,
                         "workflow_run": {"id": self.release["run_id"], "repository_id": 1,
                                          "head_repository_id": 1, "head_branch": "develop",
                                          "head_sha": self.release["source"]}}

    def fetch(self, root, mutate=None, *, repo=None):
        api = Mock()
        api.request.side_effect = [self.run, {"total_count": 1, "jobs": [self.job]}, self.artifact]

        def download(artifact, path, *_args, **_kwargs):
            self.assertEqual(artifact, self.artifact)
            producer = root / "producer"
            producer.mkdir()
            (producer / ARCHIVE).write_bytes(b"the tested OCI archive")
            seal(producer, self.release)
            native.stamp_metadata(producer, self.release, root, runner=runner)
            if mutate:
                mutate(producer)
            with zipfile.ZipFile(path, "w") as output:
                for member in producer.iterdir():
                    output.write(member, member.name)

        runner = Mock(side_effect=[b"", json.dumps([{"Id": self.image[1]}]).encode(),
                                  json.dumps({"version": self.release["version"],
                                              "source_sha": self.release["source"]}).encode(),
                                  b"", b""])
        def qualify_runtime(archive, release, digest, checksum, directory, _runner, *, source, check_stamp):
            self.assertEqual((archive, release, digest, directory, source, check_stamp),
                             (root / "producer" / ARCHIVE, self.release, self.image[0],
                              root / "producer", root, True))
            self.assertEqual(checksum, json.loads((directory / MANIFEST).read_text())["archive_sha256"])
            (directory / "runtime-smoke.json").write_text("{}")

        checkout = (Mock(return_value=f"{self.run['head_sha']}\n{self.release['tree']}\n".encode())
                    if repo is None else native.command)
        with patch.object(native, "image_identity", return_value=self.image), \
                patch.object(native, "qualify", side_effect=qualify_runtime) as qualify:
            result = native.fetch(self.release, "12", root, root, api, "fixture", root=repo or root,
                                  downloader=download, runner=checkout)
        if repo is None:
            args, kwargs = checkout.call_args
            self.assertIn(self.release["source"] + "^{tree}", args[0])
            self.assertEqual(kwargs["cwd"], root)
        if self.release["component"] == "am":
            self.assertIn(self.image[1], runner.call_args_list[2].args[0])
            qualify.assert_not_called()
        else:
            qualify.assert_called_once()
        return result

    def test_accepts_exact_attempt_without_waiting_for_enclosing_success(self):
        with tempfile.TemporaryDirectory() as temp:
            result = self.fetch(Path(temp))
            self.assertEqual(result.path.read_bytes(), b"the tested OCI archive")
            self.assertIsNone(result.cache_day)

    def test_catch_up_uses_the_reserved_first_parent_tree(self):
        with tempfile.TemporaryDirectory() as temp:
            root = Path(temp)
            repo = root / "repo"
            repo.mkdir()

            def git(*args):
                command = ["git", "-c", "commit.gpgsign=false", "-c", "user.name=Fixture",
                           "-c", "user.email=fixture@example.invalid", *args]
                return subprocess.check_output(command, cwd=repo, stderr=subprocess.PIPE, text=True).strip()

            git("init", "--quiet", "--initial-branch=develop")
            git("commit", "--quiet", "--allow-empty", "-m", "reserved source")
            reserved = git("rev-parse", "HEAD")
            git("branch", "side")
            (repo / "develop.txt").write_text("develop")
            git("add", ".")
            git("commit", "--quiet", "-m", "first parent")
            git("checkout", "--quiet", "side")
            (repo / "side.txt").write_text("side")
            git("add", ".")
            git("commit", "--quiet", "-m", "side parent")
            side = git("rev-parse", "HEAD")
            git("checkout", "--quiet", "develop")
            git("merge", "--quiet", "--no-ff", "--no-edit", "side")
            head = git("rev-parse", "HEAD")
            self.run["head_sha"] = self.job["head_sha"] = self.artifact["workflow_run"]["head_sha"] = head
            for component in ("am", "runtime", "runtime-omarchy"):
                with self.subTest(component=component), tempfile.TemporaryDirectory(dir=root) as transfer:
                    self.release.update(component=component, source=reserved, tree=git("rev-parse", reserved + "^{tree}"))
                    self.job["name"] = f"Release {component} / Native release image / Build and test"
                    self.artifact["name"] = native.artifact_name(self.release)
                    result = self.fetch(Path(transfer), repo=repo)
                    self.assertEqual(result.path.read_bytes(), b"the tested OCI archive")
                    with self.assertRaisesRegex(Refusal, "checkout or reserved tree differs"):
                        native.verify_source(repo, head, dict(self.release, tree=git("rev-parse", "HEAD^{tree}")))
                    with self.assertRaisesRegex(Refusal, "outside the push's first-parent history"):
                        native.verify_source(repo, head, dict(self.release, source=side, tree=git("rev-parse", side + "^{tree}")))
                    with self.assertRaisesRegex(Refusal, "checkout or reserved tree differs"):
                        native.verify_source(repo, reserved, self.release)
            with self.assertRaisesRegex(Refusal, "Native image component differs"):
                native.verify_source(repo, head, dict(self.release, component="web"))

    def test_refuses_artifact_from_other_attempt_or_unsuccessful_producer(self):
        for mutation in ({"run_attempt": 999}, {"conclusion": "failure"}, {"name": "Release am / Publish"}):
            with self.subTest(mutation), tempfile.TemporaryDirectory() as temp:
                original = self.job.copy()
                self.job.update(mutation)
                try:
                    with self.assertRaises(native.ArtifactRefused):
                        self.fetch(Path(temp))
                finally:
                    self.job = original

    def test_refuses_archive_whose_metadata_differs_from_the_inspected_image(self):
        def change(producer):
            path = producer / native.METADATA
            value = json.loads(path.read_text())
            value["buildFingerprint"] = "0" * 64
            path.write_text(json.dumps(value))

        with tempfile.TemporaryDirectory() as temp, self.assertRaises(Refusal):
            self.fetch(Path(temp), mutate=change)

    def test_miss_removes_only_transfer_scratch_before_fallback(self):
        with tempfile.TemporaryDirectory() as temp:
            root = Path(temp)
            work = root / "source"
            work.mkdir()
            (root / "reserved-native").mkdir()
            (root / "native.zip").touch()
            on_miss = Mock()
            with patch.object(native, "fetch", side_effect=Refusal("fixture")):
                result = native.try_fetch(self.release, "12", root, work, Mock(), "fixture", on_miss, root=root)
            self.assertIsNone(result)
            self.assertEqual(list(root.iterdir()), [work])
            on_miss.assert_called_once_with("reserved-native-unavailable")
