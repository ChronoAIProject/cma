"""Untrusted, altered or incomplete producer evidence never authorizes reuse."""
import copy
import hashlib
import io
import json
from pathlib import Path
import subprocess
import tempfile
import unittest
from unittest.mock import Mock, patch
import zipfile

import ci_retry as cache_retry
import image_detector_cache as cache


def zipped(value=b'{"schema":"fixture"}', name="cache.json"):
    stream = io.BytesIO()
    with zipfile.ZipFile(stream, "w", zipfile.ZIP_DEFLATED) as archive:
        archive.writestr(name, value)
    return stream.getvalue()


class DetectorTransportTests(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory()
        self.root = Path(self.temp.name)
        self.env = {"GITHUB_REPOSITORY": cache.REPO, "GITHUB_EVENT_NAME": "push",
                    "GITHUB_REF": "refs/heads/develop", "GITHUB_SHA": "b" * 40,
                    "GITHUB_RUN_ID": "22", "GITHUB_RUN_ATTEMPT": "1", "CD_COMPONENT": "web",
                    "RUNNER_TEMP": str(self.root), "CD_LAYER_SCAN_CACHE": "enabled"}
        self.raw = zipped()
        self.run = {"repository": {"id": 123, "full_name": cache.REPO},
                    "head_repository": {"id": 123, "full_name": cache.REPO},
                    "path": cache.WORKFLOW, "event": "push", "head_branch": "develop",
                    "status": "completed", "head_sha": "a" * 40, "id": 11, "run_attempt": 2}
        self.jobs = {"total_count": 1, "jobs": [{"name": "Release web / Publish", "status": "completed",
                      "conclusion": "success", "head_sha": "a" * 40, "run_id": 11, "run_attempt": 2}]}
        self.artifact = {"name": "cma-layer-detector-web-11-2", "expired": False, "id": 99,
                         "size_in_bytes": len(self.raw), "digest": "sha256:" + hashlib.sha256(self.raw).hexdigest(),
                         "workflow_run": {"id": 11, "head_sha": "a" * 40, "repository_id": 123, "head_repository_id": 123}}

    def tearDown(self):
        self.temp.cleanup()

    def get(self, path, **kwargs):
        self.assertGreater(kwargs["timeout"], 0)
        self.assertLessEqual(kwargs["timeout"], 10)
        if "/workflows/" in path:
            return {"workflow_runs": [self.run]}
        if "/artifacts?" in path:
            return {"total_count": 1, "artifacts": [self.artifact]}
        if "/jobs?" in path:
            return self.jobs
        if path.endswith("/zip"):
            return self.raw
        self.fail("unexpected API path")

    def restore(self, **kwargs):
        return cache.restore(self.env, self.root, self.get, kwargs.get("match_source", Mock(return_value=True)))

    def test_successful_component_can_seed_cache_after_other_component_failure(self):
        self.run["conclusion"] = "failure"
        result = self.restore()
        self.assertEqual(result["state"], "RESTORED")
        self.assertEqual(json.loads(cache.paths(self.env)[0].read_bytes()), {"schema": "fixture"})

    def test_failed_restore_leaves_no_partial_evidence_and_can_retry(self):
        destination, _ = cache.paths(self.env)
        write = Path.write_bytes

        def interrupted(path, data):
            write(path, data[:5])
            raise OSError("interrupted")

        with patch.object(Path, "write_bytes", interrupted):
            self.assertEqual(self.restore()["state"], "MISS_FULL_SCAN")
        self.assertFalse(destination.exists())
        self.assertEqual(list(self.root.iterdir()), [])
        self.assertEqual(self.restore()["state"], "RESTORED")
        self.assertEqual(destination.read_bytes(), b'{"schema":"fixture"}')

    def test_restore_never_replaces_a_racing_cache(self):
        destination, _ = cache.paths(self.env)
        link = cache.os.link

        def competing_link(candidate, path):
            path.write_bytes(b"existing")
            return link(candidate, path)

        with patch.object(cache.os, "link", competing_link):
            self.assertEqual(self.restore()["state"], "MISS_FULL_SCAN")
        self.assertEqual(destination.read_bytes(), b"existing")

    def test_live_run_requires_successful_finished_component_before_restoration(self):
        self.run.update(status="in_progress", conclusion=None)
        def get(path, **kwargs):
            if "/workflows/" in path:
                # Model the actual API filter, which used to hide this producer.
                return {"workflow_runs": [] if "status=completed" in path else [self.run]}
            return self.get(path, **kwargs)

        job = self.jobs["jobs"][0]
        for status, conclusion in (("in_progress", None), ("completed", "failure"), ("completed", "success")):
            with self.subTest(status=status, conclusion=conclusion):
                job.update(status=status, conclusion=conclusion)
                result = cache.restore(self.env, self.root, get, Mock(return_value=True))
                self.assertEqual(result["state"], "RESTORED" if conclusion == "success" else "MISS_FULL_SCAN")

    def test_transport_failures_repeat_before_falling_back_to_a_full_scan(self):
        reset = ValueError("Layer detector cache refused")
        reset.__cause__ = cache_retry.TransientError()
        failures = {"/jobs?": [reset], "/zip": [subprocess.TimeoutExpired("gh", 10)]}
        def get(path, **kwargs):
            for key, pending in failures.items():
                if key in path and pending:
                    raise pending.pop(0)
            return self.get(path, **kwargs)
        sleeps = []
        result = cache.restore(self.env, self.root, get, Mock(return_value=True), sleep=sleeps.append)
        self.assertEqual(result["state"], "RESTORED")
        self.assertEqual(len(sleeps), 2)
        with patch.object(cache.subprocess, "run",
                          return_value=subprocess.CompletedProcess([], 1, b"", b"gh: HTTP 502")), \
                self.assertRaises(ValueError) as raised:
            cache.github(f"repos/{cache.REPO}/actions/runs/1")
        self.assertEqual(cache_retry.classify(raised.exception), cache_retry.TRANSIENT)

    def test_current_run_cannot_seed_itself(self):
        result = cache.restore(dict(self.env, GITHUB_RUN_ID="11"), self.root, self.get)
        self.assertEqual(result["state"], "MISS_FULL_SCAN")

    def test_pr_manual_fork_and_wrong_ref_are_refused_before_any_read(self):
        get = Mock()
        for key, value in (("GITHUB_EVENT_NAME", "pull_request"), ("GITHUB_EVENT_NAME", "workflow_dispatch"),
                           ("GITHUB_REF", "refs/heads/feature"), ("GITHUB_REPOSITORY", "example/fork")):
            with self.subTest(key=key, value=value), self.assertRaises(ValueError):
                cache.restore(dict(self.env, **{key: value}), self.root, get)
        get.assert_not_called()

    def test_wrong_source_or_unsuccessful_publish_never_restores(self):
        self.assertEqual(self.restore(match_source=Mock(return_value=False))["state"], "MISS_FULL_SCAN")
        for conclusion in ("failure", "cancelled", "skipped", None):
            self.jobs["jobs"][0]["conclusion"] = conclusion
            self.assertEqual(self.restore()["state"], "MISS_FULL_SCAN")
        self.assertFalse(cache.paths(self.env)[0].exists())

    def test_producer_event_workflow_repo_attempt_and_head_bindings(self):
        changes = [("event", "pull_request"), ("path", ".github/workflows/ci.yml"),
                   ("head_branch", "feature"), ("status", "queued"), ("head_sha", "c" * 40),
                   ("id", 12), ("run_attempt", 3), ("repository", {"full_name": cache.REPO}),
                   ("head_repository", {"id": 456, "full_name": cache.REPO})]
        for key, value in changes:
            with self.subTest(field=key), self.assertRaises(ValueError):
                cache.validate_producer(dict(self.run, **{key: value}), self.jobs, self.artifact, "web")

    def test_artifact_and_job_association_are_exact(self):
        for key, value in (("expired", True), ("name", "cma-layer-detector-web-11-1"),
                           ("digest", "unknown"), ("size_in_bytes", cache.MAX_BYTES + 1),
                           ("workflow_run", {})):
            with self.subTest(field=key), self.assertRaises(ValueError):
                cache.validate_producer(self.run, self.jobs, dict(self.artifact, **{key: value}), "web")
        jobs = copy.deepcopy(self.jobs)
        jobs["jobs"][0]["run_attempt"] = 1
        with self.assertRaises(ValueError):
            cache.validate_producer(self.run, jobs, self.artifact, "web")
        jobs = {"jobs": self.jobs["jobs"] * 2, "total_count": 2}
        with self.assertRaises(ValueError):
            cache.validate_producer(self.run, jobs, self.artifact, "web")

    def test_omitted_findings_or_other_artifact_edits_break_authenticated_digest(self):
        original = zipped(b'{"layers":{"fixture":{"rows":[{"rule":"blocking"}]}}}')
        artifact = dict(self.artifact, digest="sha256:" + hashlib.sha256(original).hexdigest())
        with self.assertRaises(ValueError):
            cache.unpack(zipped(b'{"layers":{"fixture":{"rows":[]}}}'), artifact)

    def test_zip_member_name_duplicate_and_size_refuse(self):
        for raw in (zipped(name="../cache.json"), zipped(name="other.json")):
            artifact = dict(self.artifact, digest="sha256:" + hashlib.sha256(raw).hexdigest())
            with self.assertRaises(ValueError):
                cache.unpack(raw, artifact)
        with patch.object(cache, "MAX_BYTES", 8), self.assertRaises(ValueError):
            cache.unpack(self.raw, self.artifact)

    def test_unavailable_cache_leaves_no_input_and_never_logs_exception(self):
        with patch.object(cache, "restore", side_effect=OSError("private-material")), patch("sys.stdout", new_callable=io.StringIO) as output:
            cache.main()
        self.assertEqual(json.loads(output.getvalue())["state"], "UNAVAILABLE_FULL_SCAN")
        self.assertNotIn("private-material", output.getvalue())
        self.assertFalse(cache.paths(self.env)[0].exists())

    def test_scanner_options_bind_catalog_and_component_and_keep_credentials_absent(self):
        for name in cache.PRODUCER_FILES:
            target = self.root / name
            target.parent.mkdir(parents=True, exist_ok=True)
            target.write_text("synthetic-policy")
        first = cache.scanner_options(self.root, self.env, "credential", None)
        self.assertNotIn("--detector-cache-input", first)
        self.assertEqual(cache.scanner_options(self.root, {}, "credential", None), [])
        other = cache.scanner_options(self.root, dict(self.env, CD_COMPONENT="am"), "credential", None)
        self.assertNotEqual(first[1], other[1])
        catalog = self.root / ".github/scripts/image-content-manifest.json"
        catalog.write_text("changed-policy")
        self.assertNotEqual(first[1], cache.scanner_options(self.root, self.env, "credential", None)[1])
        self.assertNotIn("GH_TOKEN", " ".join(first))
        source, destination = cache.paths(self.env)
        source.write_text("{}")
        self.assertIn(str(source), cache.scanner_options(self.root, self.env, "credential", None))
        destination.parent.mkdir()
        destination.write_text("{}")
        self.assertIn(str(destination), cache.scanner_options(self.root, self.env, "credential", None)[-1])

    def test_real_git_source_must_be_ancestor_with_identical_producer_files(self):
        def git(*args):
            return subprocess.check_output(["git", "-c", "user.name=Fixture", "-c", "user.email=fixture@example.invalid", *args],
                                           cwd=self.root, stderr=subprocess.DEVNULL).decode().strip()
        git("init")
        for name in cache.PRODUCER_FILES:
            path = self.root / name
            path.parent.mkdir(parents=True, exist_ok=True)
            path.write_text("original")
        git("add", ".")
        git("commit", "-m", "fixture")
        source = git("rev-parse", "HEAD")
        self.assertTrue(cache.source_matches(self.root, source, source))
        (self.root / cache.PRODUCER_FILES[0]).write_text("changed")
        self.assertFalse(cache.source_matches(self.root, source, source))
        self.assertFalse(cache.source_matches(self.root, "d" * 40, source))

    def test_workflow_only_restores_and_exports_on_protected_publish_job(self):
        text = (Path(__file__).resolve().parents[1] / "workflows/staging-release.yml").read_text()
        publisher = text.split("\n  publish:\n", 1)[1].split("\n  finalize:\n", 1)[0]
        self.assertIn("github.event_name == 'push' && github.ref == 'refs/heads/develop'", publisher)
        self.assertIn("github.repository == 'ChronoAIProject/cma'", publisher)
        self.assertLess(text.index("Restore authenticated layer detector evidence"), text.index("Build once, scan every layer"))
        self.assertLess(text.index("Build once, scan every layer"), text.index("Retain complete layer detector evidence"))
        export = text.split("- name: Retain complete layer detector evidence", 1)[1].split("- name:", 1)[0]
        self.assertIn("if: success()", export)
        self.assertIn("actions/upload-artifact@cf430e030ddbb5b0abf93d22962f4752f3646cd9", export)
        self.assertNotIn("restore-keys", export)


if __name__ == "__main__":
    unittest.main()
