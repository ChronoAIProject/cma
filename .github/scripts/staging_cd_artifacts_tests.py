"""PR artifacts are reusable only with exact source, recipe and checksum evidence."""
import hashlib
import json
import subprocess
import tempfile
import unittest
import zipfile
from pathlib import Path
from unittest.mock import Mock, patch

import ci_retry
from staging_cd_artifacts import REUSE_SECONDS, download, qualifies, try_reuse
from staging_cd_common import Refusal
from staging_cd_receipts_tests import fixture_identity


class ArtifactTests(unittest.TestCase):
    @staticmethod
    def convert(command, **_options):
        Path(command[-1].removeprefix("oci-archive:")).write_bytes(b"synthetic OCI image")

    def fixture(self):
        expected = fixture_identity()
        run = {"id": 10, "run_attempt": 1, "event": "pull_request", "status": "completed", "conclusion": "success",
               "head_sha": "f" * 40, "path": ".github/workflows/ci.yml", "repository": {"full_name": "ChronoAIProject/cma"}}
        image = b"synthetic tested image"
        metadata = {"schema": "cma-ci-native-image-v1", "component": "web", "sourceRevision": expected["source"],
                    "sourceTree": expected["tree"], "platform": "linux/amd64", "versionLabel": expected["version"],
                    "buildFingerprint": expected["build"], "archive": "cma-web-linux-amd64.docker.tar.gz",
                    "archiveSha256": hashlib.sha256(image).hexdigest(), "archiveBytes": len(image),
                    "workflow": {"repository": "ChronoAIProject/cma", "event": "pull_request", "runId": "10", "runAttempt": "1"}}
        api = Mock()
        artifact = {"id": 20, "name": "ci-native-image-web-" + "a" * 40 + "-10-1", "expired": False}
        api.request.side_effect = [{"merged": True, "merge_commit_sha": expected["source"], "head": {"sha": "f" * 40}},
                                   {"workflow_runs": [run]}, {"total_count": 1, "artifacts": [artifact]}]
        def download(_artifact, destination, _token, **_options):
            with zipfile.ZipFile(destination, "w") as output:
                output.writestr("metadata.json", json.dumps(metadata))
                output.writestr(metadata["archive"], image)
        return expected, run, metadata, api, download

    def queue_fixture(self):
        """A develop merge-queue run whose group commit is the reserved source."""
        expected, _, metadata, _, download = self.fixture()
        branch = "gh-readonly-queue/develop/pr-42-" + "e" * 40
        run = {"id": 30, "run_attempt": 2, "event": "merge_group", "status": "completed", "conclusion": "success",
               "head_sha": expected["source"], "head_branch": branch, "path": ".github/workflows/ci.yml",
               "repository": {"full_name": "ChronoAIProject/cma"},
               "head_repository": {"full_name": "ChronoAIProject/cma"}}
        metadata["workflow"] = {"repository": "ChronoAIProject/cma", "event": "merge_group", "runId": "30",
                                "runAttempt": "2", "ref": "refs/heads/" + branch}
        pr = {"merged": True, "merge_commit_sha": expected["source"], "head": {
            "sha": "f" * 40, "repo": {"full_name": "ChronoAIProject/cma"}}, "base": {"ref": "develop"}}
        artifact = {"id": 40, "name": "ci-native-image-web-" + expected["source"] + "-30-2", "expired": False}
        api = Mock()
        api.request.side_effect = [pr, {"workflow_runs": [run]}, {"total_count": 1, "artifacts": [artifact]}]
        return expected, run, metadata, api, download, pr

    def test_exact_develop_queue_run_is_adopted_before_any_pr_run(self):
        expected, run, metadata, api, download, _ = self.queue_fixture()
        self.assertTrue(qualifies(metadata, expected, run))
        with tempfile.TemporaryDirectory() as directory, patch("staging_cd_artifacts.inspect") as inspect:
            path = try_reuse(expected, Path(directory), api, "fixture", download, self.convert)
        inspect.assert_called_once()
        self.assertEqual(path.name, "image.oci.tar")
        paths = [call.args[0] for call in api.request.call_args_list]
        self.assertEqual(paths, ["pulls/42",
                                 "actions/workflows/ci.yml/runs?event=merge_group&head_sha=" + expected["source"]
                                 + "&status=success&per_page=100", "actions/runs/30/artifacts?per_page=100"])

    def test_queue_manifest_must_name_its_own_run_event_and_group_ref(self):
        expected, run, metadata, *_ = self.queue_fixture()
        for changes in ({"event": "pull_request"}, {"event": "push"}, {"ref": "refs/heads/develop"},
                        {"ref": "refs/heads/gh-readonly-queue/develop/pr-43-" + "e" * 40}, {"runAttempt": "1"}):
            with self.subTest(changes=changes):
                self.assertFalse(qualifies(dict(metadata, workflow=dict(metadata["workflow"], **changes)), expected, run))
        # A PR manifest cannot be adopted through a queue run, nor the reverse.
        _, pr_run, pr_metadata, *_ = self.fixture()
        self.assertFalse(qualifies(pr_metadata, expected, dict(pr_run, event="merge_group")))
        self.assertFalse(qualifies(metadata, expected, dict(run, event="push")))

    def test_only_same_repository_develop_queue_runs_for_the_exact_source_are_candidates(self):
        for changes in ({"event": "push"}, {"head_sha": "d" * 40}, {"conclusion": "failure"}, {"status": "in_progress"},
                        {"path": ".github/workflows/staging-cd.yml"}, {"repository": {"full_name": "fork/cma"}},
                        {"head_repository": {"full_name": "fork/cma"}}, {"head_repository": None},
                        {"head_branch": "gh-readonly-queue/main/pr-42-" + "e" * 40},
                        {"head_branch": "gh-readonly-queue/develop/pr-43-" + "e" * 40},
                        {"head_branch": "gh-readonly-queue/develop/pr-42-short"}, {"head_branch": "develop"},
                        {"id": "30"}):
            expected, run, _, api, _, pr = self.queue_fixture()
            run.update(changes)
            # Rejected queue runs fall through to the PR lookup, which finds nothing.
            api.request.side_effect = [pr, {"workflow_runs": [run]}, {"workflow_runs": []}]
            with self.subTest(changes=changes), tempfile.TemporaryDirectory() as directory:
                downloader = Mock()
                self.assertIsNone(try_reuse(expected, Path(directory), api, "fixture", downloader, Mock()))
                downloader.assert_not_called()
                self.assertEqual(api.request.call_args_list[-1].args[0],
                                 "actions/runs?event=pull_request&head_sha=" + "f" * 40
                                 + "&status=success&per_page=100")

    def test_fork_or_non_develop_prs_never_consult_queue_artifacts(self):
        for head, base in (({"sha": "f" * 40, "repo": {"full_name": "fork/cma"}}, {"ref": "develop"}),
                           ({"sha": "f" * 40, "repo": None}, {"ref": "develop"}),
                           ({"sha": "f" * 40, "repo": {"full_name": "ChronoAIProject/cma"}}, {"ref": "main"})):
            expected, _, _, api, _, pr = self.queue_fixture()
            api.request.side_effect = [dict(pr, head=head, base=base), {"workflow_runs": []}]
            with self.subTest(head=head, base=base), tempfile.TemporaryDirectory() as directory:
                self.assertIsNone(try_reuse(expected, Path(directory), api, "fixture", Mock(), Mock()))
                self.assertEqual([call.args[0] for call in api.request.call_args_list],
                                 ["pulls/42", "actions/runs?event=pull_request&head_sha=" + "f" * 40
                                  + "&status=success&per_page=100"])

    def test_queue_run_without_this_component_falls_back_to_the_pr_run(self):
        expected, queue, _, api, _, pr = self.queue_fixture()
        _, pr_run, metadata, _, download = self.fixture()
        artifact = {"id": 20, "name": "ci-native-image-web-" + "a" * 40 + "-10-1", "expired": False}
        api.request.side_effect = [pr, {"workflow_runs": [queue]}, {"total_count": 0, "artifacts": []},
                                   {"workflow_runs": [pr_run]}, {"total_count": 1, "artifacts": [artifact]}]
        with tempfile.TemporaryDirectory() as directory, patch("staging_cd_artifacts.inspect"):
            self.assertIsNotNone(try_reuse(expected, Path(directory), api, "fixture", download, self.convert))
        self.assertEqual(api.request.call_args_list[-1].args[0], "actions/runs/10/artifacts?per_page=100")

    def test_current_ci_manifests_cannot_qualify_until_they_bind_the_release_build(self):
        # Today's exporter writes the CI version label and no build fingerprint;
        # even an exact queue run therefore falls back to a build from source.
        expected, run, metadata, *_ = self.queue_fixture()
        current = dict(metadata, versionLabel="git-" + expected["source"][:12])
        current.pop("buildFingerprint")
        self.assertFalse(qualifies(current, expected, run))

    def test_current_pr_manifests_and_changed_version_or_recipe_are_ineligible(self):
        expected, run, metadata, _, _ = self.fixture()
        self.assertTrue(qualifies(metadata, expected, run))
        legacy = dict(metadata)
        legacy.pop("buildFingerprint")
        self.assertFalse(qualifies(legacy, expected, run))
        for field, value in (("sourceTree", "d" * 40), ("sourceRevision", "d" * 40), ("platform", "linux/arm64"),
                             ("versionLabel", "git-fixture"), ("buildFingerprint", "d" * 64), ("component", "api")):
            with self.subTest(field=field):
                self.assertFalse(qualifies(dict(metadata, **{field: value}), expected, run))

    def test_qualified_artifact_is_converted_and_validated_without_building(self):
        expected, _, _, api, download = self.fixture()
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            runner = Mock(side_effect=self.convert)
            with patch("staging_cd_artifacts.inspect") as inspect:
                path = try_reuse(expected, root, api, "fixture", download, runner)
            self.assertEqual(path, root / "image.oci.tar")
            self.assertEqual(runner.call_count, 1)
            self.assertEqual(runner.call_args.args[0][:2], ["skopeo", "copy"])
            inspected_path, identity, _ = inspect.call_args.args
            self.assertEqual(inspected_path.name, path.name)
            self.assertEqual(identity, expected)
            self.assertEqual(inspect.call_args.kwargs, {"allow_qualified": True, "compatibility": None})
            self.assertEqual(list(root.iterdir()), [path])

    def test_split_pr_artifact_uses_only_the_component_workflow(self):
        expected, run, _, api, download = self.fixture()
        run["path"] = ".github/workflows/ci-pr-web.yml"
        responses = list(api.request.side_effect)
        responses[1]["workflow_runs"].append(
            dict(run, id=11, path=".github/workflows/ci-pr-eg.yml")
        )
        api.request.side_effect = responses
        with tempfile.TemporaryDirectory() as directory, patch("staging_cd_artifacts.inspect"):
            self.assertIsNotNone(try_reuse(expected, Path(directory), api, "fixture", download, self.convert))
        self.assertEqual(api.request.call_args_list[-1].args[0], "actions/runs/10/artifacts?per_page=100")

    def test_unqualified_manifest_falls_back_before_using_image_archive(self):
        expected, _, metadata, api, download = self.fixture()
        metadata.pop("buildFingerprint")
        with tempfile.TemporaryDirectory() as directory:
            runner = Mock()
            misses = []
            self.assertIsNone(try_reuse(expected, Path(directory), api, "fixture", download, runner,
                                        on_miss=misses.append))
            self.assertEqual(misses, ["manifest:buildFingerprint"])
            runner.assert_not_called()
            self.assertFalse((Path(directory) / "qualified.docker.tar.gz").exists())

    def test_optional_candidate_lookup_or_download_failure_falls_back(self):
        transient = Refusal("GitHub API request failed")
        transient.__cause__ = ci_retry.TransientError()
        cases = (("api", transient, "unreachable"),
                 ("api", Refusal("GitHub API request failed"), "candidate-unavailable"),
                 ("download", subprocess.TimeoutExpired("gh", 600), "unreachable"),
                 ("download", Refusal("CI artifact checksum differs"), "candidate-unavailable"))
        for stage, error, reason in cases:
            with self.subTest(stage=stage, reason=reason), tempfile.TemporaryDirectory() as directory:
                expected, _, _, api, download = self.fixture()
                if stage == "api":
                    api.request.side_effect = error
                else:
                    download = Mock(side_effect=error)
                runner, misses = Mock(), []
                root = Path(directory)
                self.assertIsNone(try_reuse(expected, root, api, "fixture", download, runner, on_miss=misses.append))
                self.assertEqual(misses, [reason])
                self.assertEqual(list(root.iterdir()), [])
                runner.assert_not_called()

    def test_different_merge_preview_never_downloads_its_archive(self):
        expected, _, _, api, _ = self.fixture()
        pr, runs, listing = list(api.request.side_effect)
        listing["artifacts"][0]["name"] = "ci-native-image-web-" + "d" * 40 + "-10-1"
        api.request.side_effect = [pr, runs, listing]
        download, misses = Mock(), []
        with tempfile.TemporaryDirectory() as directory:
            self.assertIsNone(try_reuse(expected, Path(directory), api, "fixture", download, on_miss=misses.append))
        self.assertEqual(misses, ["no-artifact"])
        download.assert_not_called()

    def test_reuse_reads_and_download_share_one_budget(self):
        expected, _, _, api, download = self.fixture()
        seen = []
        def recording(artifact, destination, token, *, deadline):
            seen.append(deadline)
            download(artifact, destination, token)
        with tempfile.TemporaryDirectory() as directory, patch("staging_cd_artifacts.inspect"):
            try_reuse(expected, Path(directory), api, "fixture", recording, self.convert, clock=lambda: 1000.0)
        deadline = 1000.0 + REUSE_SECONDS
        self.assertEqual([call.kwargs["deadline"] for call in api.request.call_args_list], [deadline] * 3)
        self.assertEqual(seen, [deadline])
        # Once the budget is spent the probe stops reading and the release builds.
        expected, _, _, api, _ = self.fixture()
        spent = Refusal("GitHub read budget expired")
        spent.__cause__ = ci_retry.TransientError()
        api.request.side_effect = spent
        with tempfile.TemporaryDirectory() as directory:
            self.assertIsNone(try_reuse(expected, Path(directory), api, "fixture", Mock(), Mock()))

    def test_download_attempts_are_shortened_to_the_shared_budget(self):
        content = b"complete artifact"
        artifact = {"id": 20, "size_in_bytes": len(content),
                    "digest": "sha256:" + hashlib.sha256(content).hexdigest()}
        now, timeouts = [0.0], []
        def run(args, *, stdout, timeout, **_kwargs):
            timeouts.append(timeout)
            now[0] += 500
            raise subprocess.TimeoutExpired(args, timeout)
        policy = {"clock": lambda: now[0], "sleep": lambda seconds: now.__setitem__(0, now[0] + seconds),
                  "rand": lambda: 1.0}
        with tempfile.TemporaryDirectory() as directory, \
                patch("staging_cd_artifacts.subprocess.run", side_effect=run), \
                self.assertRaises(subprocess.TimeoutExpired) as raised:
            download(artifact, Path(directory) / "candidate.zip", "fixture", deadline=800.0, retry=policy)
        # 600 s at most, then only what remains after the wait; never 3 x 600 s.
        self.assertEqual(timeouts, [600, 299.0])
        self.assertEqual(ci_retry.classify(raised.exception), ci_retry.TRANSIENT)
        with tempfile.TemporaryDirectory() as directory, \
                patch("staging_cd_artifacts.subprocess.run") as untouched, \
                self.assertRaisesRegex(Refusal, "budget expired") as raised:
            download(artifact, Path(directory) / "candidate.zip", "fixture", deadline=now[0], retry=policy)
        untouched.assert_not_called()
        self.assertEqual(ci_retry.classify(raised.exception), ci_retry.TRANSIENT)

    def test_download_repeats_transport_failures_without_keeping_partial_bytes(self):
        content = b"complete artifact"
        artifact = {"id": 20, "size_in_bytes": len(content),
                    "digest": "sha256:" + hashlib.sha256(content).hexdigest()}
        def scripted(*outcomes):
            def run(args, *, stdout, **_kwargs):
                body, code, stderr = outcomes[run.calls]
                run.calls += 1
                stdout.write(body)
                return subprocess.CompletedProcess(args, code, None, stderr)
            run.calls = 0
            return run
        with tempfile.TemporaryDirectory() as directory:
            target = Path(directory) / "candidate.zip"
            run = scripted((b"compl", 1, b"read: connection reset by peer"), (content, 0, b""))
            with patch("staging_cd_artifacts.subprocess.run", side_effect=run):
                download(artifact, target, "fixture", retry={"sleep": lambda _seconds: None})
            self.assertEqual((run.calls, target.read_bytes()), (2, content))
            target.unlink()
            run = scripted((b"", 1, b"gh: Not Found (HTTP 404)"), (content, 0, b""))
            with patch("staging_cd_artifacts.subprocess.run", side_effect=run), \
                    self.assertRaisesRegex(Refusal, "download failed"):
                download(artifact, target, "fixture", retry={"sleep": lambda _seconds: None})
            self.assertEqual(run.calls, 1)
            self.assertFalse(target.exists())
            # A file the download did not create is never adopted or removed.
            target.write_bytes(b"foreign")
            with patch("staging_cd_artifacts.subprocess.run") as untouched, self.assertRaises(FileExistsError):
                download(artifact, target, "fixture")
            untouched.assert_not_called()
            self.assertEqual(target.read_bytes(), b"foreign")

    def test_unreadable_or_substituted_archive_never_converts(self):
        expected, _, metadata, api, download = self.fixture()
        metadata["archiveSha256"] = "0" * 64

        def corrupt(_artifact, destination, _token, **_options):
            destination.write_bytes(b"not a zip file")

        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            runner, misses = Mock(), []
            self.assertIsNone(try_reuse(expected, root, api, "fixture", download, runner, on_miss=misses.append))
            expected, _, _, api, _ = self.fixture()
            self.assertIsNone(try_reuse(expected, root, api, "fixture", corrupt, runner, on_miss=misses.append))
            self.assertEqual(misses, ["candidate-invalid", "candidate-invalid"])
            self.assertEqual(list(root.iterdir()), [])
            runner.assert_not_called()

    def test_invalid_oci_or_expired_conversion_keeps_no_candidate(self):
        for expires in (False, True):
            with self.subTest(expires=expires), tempfile.TemporaryDirectory() as directory:
                expected, _, _, api, download = self.fixture()
                root = Path(directory)
                source_root = root / "source"
                misses = []
                now = 1000.0

                def copy(command, *, timeout):
                    nonlocal now
                    self.assertEqual(timeout, 600)
                    self.convert(command)
                    if expires:
                        now += REUSE_SECONDS

                def inspect(path, _expected, runner, *, allow_qualified, compatibility):
                    self.assertTrue(allow_qualified)
                    self.assertEqual(compatibility, {"fixture": "trusted release proof"})
                    if expires:
                        runner(["skopeo", "inspect"], timeout=120)
                    raise Refusal("Native image platform is invalid")

                with patch("staging_cd_artifacts.inspect", side_effect=inspect), \
                        patch("staging_cd_artifacts.from_source", return_value={"fixture": "trusted release proof"}) as contract:
                    self.assertIsNone(try_reuse(expected, root, api, "fixture", download, copy, clock=lambda: now,
                                                on_miss=misses.append, source_root=source_root))
                contract.assert_called_once_with(expected["component"], expected["source"], source_root)
                self.assertEqual(misses, ["candidate-invalid"])
                self.assertEqual(list(root.iterdir()), [])


if __name__ == "__main__":
    unittest.main()
