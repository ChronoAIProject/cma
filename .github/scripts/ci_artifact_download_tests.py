"""A stalled or broken artifact download is stopped and repeated; only verified bytes are extracted."""
import hashlib
import io
import json
import os
import shutil
import stat
import sys
import tempfile
import time
import unittest
import unittest.mock
import zipfile
from pathlib import Path

import yaml

import ci_artifact_download as restore_module
import ci_retry

NAME = "rust-tests-" + "a" * 40
REPOSITORY = "example-org/example-repo"
RUN_ID = 7
ARCHIVE = b"synthetic nextest archive\n" * 64
ROOT = Path(__file__).resolve().parents[2]

# The fake reads its plan from HOME, the one location the real environment
# filter passes through, and logs every call so each test can count attempts.
# Like `gh api --include`, it writes the final status line and headers before
# the body; a step with "headers": null fails before any response arrives.
FAKE_GH = r'''
import json, os, sys, time
home = os.environ["HOME"]
with open(os.path.join(home, "plan.json")) as stream:
    plan = json.load(stream)
included = sys.argv[1:3] == ["api", "--include"] and len(sys.argv) == 4
path = sys.argv[3] if included else ""
kind = "zip" if path.endswith("/zip") else "list"
with open(os.path.join(home, "calls.jsonl"), "a") as stream:
    stream.write(json.dumps({"kind": kind, "path": path, "token": os.environ.get("GH_TOKEN"),
                             "host": os.environ.get("GH_HOST"), "leak": "FAKE_PRIVATE" in os.environ}) + "\n")
with open(os.path.join(home, "calls.jsonl")) as stream:
    count = sum(1 for line in stream if json.loads(line)["kind"] == kind)
steps = plan[kind]
step = steps[min(count, len(steps)) - 1]
out = sys.stdout.buffer
headers = step.get("headers", {"Content-Type": "application/octet-stream"})
if headers is not None:
    out.write(f"HTTP/2.0 {step.get('status', 200)} Fixture\n".encode())
    out.write("".join(f"{key}: {value}\r\n" for key, value in headers.items()).encode() + b"\r\n")
if "file" in step:
    with open(step["file"], "rb") as stream:
        data = stream.read()
    out.write(data[:step["partial"]] if "partial" in step else data)
if "text" in step:
    out.write(step["text"].encode())
out.flush()
time.sleep(step.get("sleep", 0))
sys.stderr.write(step.get("stderr", ""))
sys.exit(step.get("exit", 0))
'''


def archive_zip(entries):
    buffer = io.BytesIO()
    with zipfile.ZipFile(buffer, "w") as target:
        for name, data in entries:
            if isinstance(data, zipfile.ZipInfo):
                target.writestr(data, name)
            else:
                target.writestr(name, data)
    return buffer.getvalue()


def symlink_entry(name):
    info = zipfile.ZipInfo(name)
    info.external_attr = (stat.S_IFLNK | 0o777) << 16
    return info


class RestoreTests(unittest.TestCase):
    def setUp(self):
        self.root = Path(tempfile.mkdtemp(prefix="artifact-download-tests-"))
        self.addCleanup(shutil.rmtree, self.root, ignore_errors=True)
        self.home, self.bin, self.destination = self.root / "home", self.root / "bin", self.root / "temp"
        self.home.mkdir()
        self.bin.mkdir()
        fake = self.bin / "gh"
        fake.write_text(f"#!{sys.executable}\n{FAKE_GH}")
        fake.chmod(0o755)
        self.env = {"PATH": f"{self.bin}{os.pathsep}{os.environ.get('PATH', '')}", "HOME": str(self.home),
                    "GH_TOKEN": "synthetic-token", "GH_HOST": "github.com"}
        self.retries = []

    def artifact(self, payload, **changes):
        item = {"id": 41, "name": NAME, "expired": False, "size_in_bytes": len(payload),
                "digest": "sha256:" + hashlib.sha256(payload).hexdigest(), "workflow_run": {"id": RUN_ID}}
        item.update(changes)
        return {key: value for key, value in item.items() if value is not None}

    def plan(self, payload, zips, artifacts=None, lists=("ok",)):
        source = self.root / "artifact.zip"
        source.write_bytes(payload)
        artifacts = [self.artifact(payload)] if artifacts is None else artifacts
        listing = {"text": json.dumps({"total_count": len(artifacts), "artifacts": artifacts})}
        steps = [{"file": str(source)} if step == "ok" else dict(step, file=str(source)) if "partial" in step else step
                 for step in zips]
        (self.home / "plan.json").write_text(json.dumps({"list": [listing if step == "ok" else step for step in lists],
                                                         "zip": steps}))
        (self.home / "calls.jsonl").unlink(missing_ok=True)

    def calls(self, kind):
        log = self.home / "calls.jsonl"
        rows = [json.loads(line) for line in log.read_text().splitlines()] if log.exists() else []
        return [row for row in rows if row["kind"] == kind]

    def restore(self, name=NAME, **bounds):
        out = io.StringIO()
        bounds.setdefault("attempt_seconds", 5)
        status = restore_module.restore(name, self.destination, repository=REPOSITORY, run_id=RUN_ID, env=self.env,
                                        out=out, retry={"sleep": lambda _seconds: None, "log": self.retries.append},
                                        **bounds)
        return status, out.getvalue()

    def assert_restored(self, status, output):
        self.assertEqual(status, 0, output)
        self.assertEqual([path.name for path in self.destination.iterdir()], ["backend-tests.tar.zst"],
                         "no partial download or work directory may remain")
        self.assertEqual((self.destination / "backend-tests.tar.zst").read_bytes(), ARCHIVE)

    def assert_nothing_extracted(self):
        self.assertEqual(sorted(self.destination.iterdir()) if self.destination.exists() else [], [])

    def test_verified_archive_lands_where_the_action_put_it(self):
        payload = archive_zip([("backend-tests.tar.zst", ARCHIVE)])
        older = self.artifact(payload, id=40, digest="sha256:" + "0" * 64)
        self.plan(payload, ["ok"], artifacts=[older, self.artifact(payload), self.artifact(payload, id=99, name="other"),
                                              self.artifact(payload, id=98, expired=True)])
        status, output = self.restore()
        self.assert_restored(status, output)
        self.assertEqual([call["path"] for call in self.calls("list")],
                         [f"repos/{REPOSITORY}/actions/runs/{RUN_ID}/artifacts?name={NAME}&per_page=100"])
        self.assertEqual([call["path"] for call in self.calls("zip")], [f"repos/{REPOSITORY}/actions/artifacts/41/zip"])
        self.assertIn(f"Restored {NAME}: {len(payload)} bytes matching sha256:", output)
        self.assertNotIn("::error", output)

    def test_a_stalled_attempt_is_stopped_and_repeated_on_a_fresh_connection(self):
        payload = archive_zip([("backend-tests.tar.zst", ARCHIVE)])
        self.plan(payload, [{"partial": 100, "sleep": 30}, "ok"])
        started = time.monotonic()
        status, output = self.restore(attempt_seconds=1)
        self.assertLess(time.monotonic() - started, 15, "a stalled attempt must be stopped at its bound")
        self.assert_restored(status, output)
        self.assertEqual(len(self.calls("zip")), 2)
        self.assertIn(f"::warning title=Artifact download attempt timed out::{NAME}: attempt 1 of 3 exceeded "
                      "its 1-second bound", output)
        self.assertEqual(len(self.retries), 1)
        self.assertTrue(self.retries[0].startswith("RETRY op=artifact-download attempt=2/3 "))

    def test_a_stream_that_breaks_mid_download_is_repeated(self):
        # gh reports a dropped blob connection as a bare "unexpected EOF".
        payload = archive_zip([("backend-tests.tar.zst", ARCHIVE)])
        self.plan(payload, [{"partial": 100, "stderr": "unexpected EOF", "exit": 1}, "ok"])
        status, output = self.restore()
        self.assert_restored(status, output)
        self.assertEqual(len(self.calls("zip")), 2)

    def test_a_connection_that_fails_before_any_response_is_repeated(self):
        payload = archive_zip([("backend-tests.tar.zst", ARCHIVE)])
        broken = {"headers": None, "stderr": "Get: read: connection reset by peer", "exit": 1}
        self.plan(payload, [{"headers": None, "stderr": "unexpected EOF", "exit": 1}, "ok"], lists=(broken, "ok"))
        status, output = self.restore()
        self.assert_restored(status, output)
        self.assertEqual((len(self.calls("list")), len(self.calls("zip"))), (2, 2))

    def test_timing_out_every_attempt_fails_naming_the_artifact_and_the_count(self):
        payload = archive_zip([("backend-tests.tar.zst", ARCHIVE)])
        self.plan(payload, [{"partial": 100, "sleep": 30}])
        status, output = self.restore(attempt_seconds=1)
        self.assertEqual(status, 1)
        self.assertEqual(len(self.calls("zip")), 3)
        self.assert_nothing_extracted()
        self.assertIn(f"::error title=Artifact download failed::{NAME} was not downloaded: it timed out 3 times, "
                      "each attempt stopped after 1 seconds.", output)
        machine = [line for line in output.splitlines() if line.startswith("FAIL ")]
        self.assertEqual(len(machine), 1)
        self.assertRegex(machine[0], r"^FAIL artifact-download reason=timeout at=ci_artifact_download\.py:\d+ "
                                     r"attempts=3 cause=transient$")
        self.assertIn("title=artifact-download%3A Step timed out", output)

    def test_http_errors_are_classified_by_status_and_headers(self):
        payload = archive_zip([("backend-tests.tar.zst", ARCHIVE)])
        self.plan(payload, [{"status": 502, "stderr": "gh: Server Error (HTTP 502)", "exit": 1}, "ok"])
        self.assert_restored(*self.restore())
        self.assertEqual(len(self.calls("zip")), 2)
        shutil.rmtree(self.destination)
        self.plan(payload, [{"status": 404, "stderr": "gh: Not Found (HTTP 404)", "exit": 1}, "ok"])
        status, output = self.restore()
        self.assertEqual((status, len(self.calls("zip"))), (1, 1), "a client error is not repeated")
        self.assert_nothing_extracted()
        self.assertIn(f"{NAME} was not downloaded after 1 of at most 3 attempts, 0 of them stopped", output)
        self.assertRegex(output, r"FAIL artifact-download reason=http-client at=ci_artifact_download\.py:\d+ attempts=1")

    def test_a_refused_token_names_the_missing_permission_and_is_not_repeated(self):
        payload = archive_zip([("backend-tests.tar.zst", ARCHIVE)])
        denied = {"status": 403, "stderr": "gh: Resource not accessible by integration (HTTP 403)", "exit": 1,
                  "headers": {"X-RateLimit-Remaining": "4999"}}
        self.plan(payload, ["ok"], lists=(denied, "ok"))
        status, output = self.restore()
        self.assertEqual((status, len(self.calls("list")), self.calls("zip")), (1, 1, []))
        self.assertIn(f"GitHub refused this job's token for {NAME} (HTTP 403). The job must grant actions: read", output)
        self.assertRegex(output, r"FAIL artifact-download reason=http-denied at=ci_artifact_download\.py:\d+ cause=operator")

    def test_a_throttled_403_stays_transient(self):
        payload = archive_zip([("backend-tests.tar.zst", ARCHIVE)])
        for headers in ({"X-RateLimit-Remaining": "0"}, {"Retry-After": "1"}):
            with self.subTest(headers=headers):
                throttled = {"status": 403, "stderr": "gh: API rate limit exceeded (HTTP 403)", "exit": 1,
                             "headers": headers}
                self.plan(payload, [throttled, "ok"], lists=(throttled, "ok"))
                self.assert_restored(*self.restore())
                self.assertEqual((len(self.calls("list")), len(self.calls("zip"))), (2, 2))
                shutil.rmtree(self.destination)

    def test_bytes_that_differ_from_the_recorded_digest_or_size_are_never_extracted(self):
        payload = archive_zip([("backend-tests.tar.zst", ARCHIVE)])
        for change in ({"digest": "sha256:" + "f" * 64}, {"size_in_bytes": len(payload) + 1}):
            with self.subTest(change=change):
                self.plan(payload, ["ok"], artifacts=[self.artifact(payload, **change)])
                status, output = self.restore()
                self.assertEqual((status, len(self.calls("zip"))), (1, 1), "a mismatch is not repeated")
                self.assert_nothing_extracted()
                self.assertIn("differs from the record GitHub kept at upload. Nothing was extracted.", output)
                self.assertIn("reason=refused", output)

    def test_a_body_larger_than_the_recorded_size_is_cut_off(self):
        payload = archive_zip([("backend-tests.tar.zst", ARCHIVE)])
        self.plan(payload, ["ok"], artifacts=[self.artifact(payload, size_in_bytes=len(payload) - 1)])
        status, output = self.restore()
        self.assertEqual((status, len(self.calls("zip"))), (1, 1))
        self.assert_nothing_extracted()
        self.assertIn(f"{NAME} was not downloaded after 1 of at most 3 attempts", output, "stopped while downloading")

    def test_unsafe_entries_are_refused_before_any_file_is_written(self):
        for entry in (("../escape", b"x"), ("/absolute", b"x"), ("link", symlink_entry("link")),
                      ("dir\\file", b"x")):
            with self.subTest(entry=entry[0]):
                payload = archive_zip([("backend-tests.tar.zst", ARCHIVE), entry])
                self.plan(payload, ["ok"])
                status, output = self.restore()
                self.assertEqual(status, 1)
                self.assert_nothing_extracted()
                self.assertFalse((self.root / "escape").exists())
                self.assertIn("matched its recorded digest but could not be extracted", output)

    def test_an_expired_artifact_is_final_and_asks_for_a_full_re_run(self):
        payload = archive_zip([("backend-tests.tar.zst", ARCHIVE)])
        self.plan(payload, ["ok"], artifacts=[self.artifact(payload, expired=True)])
        status, output = self.restore()
        self.assertEqual((status, len(self.calls("list")), self.calls("zip")), (1, 1, []), "expiry is not repeated")
        self.assertIn(f"{NAME} has expired with its retention period. Re-run all jobs", output)
        self.assertIn("reason=refused", output)

    def test_an_absent_or_undigested_artifact_is_never_downloaded(self):
        payload = archive_zip([("backend-tests.tar.zst", ARCHIVE)])
        self.plan(payload, ["ok"], artifacts=[self.artifact(payload, digest=None)])
        status, output = self.restore()
        self.assertEqual((status, self.calls("zip")), (1, []))
        self.assertIn(f"could not be listed, or {NAME} has no recorded SHA-256 digest", output)
        self.plan(payload, ["ok"], artifacts=[self.artifact(payload, workflow_run={"id": RUN_ID + 1})])
        status, output = self.restore()
        self.assertEqual((status, len(self.calls("list")), self.calls("zip")), (1, 3, []), "a lagging listing is reread")
        self.assertIn(f"{NAME} is not listed in this workflow run", output)
        self.assert_nothing_extracted()

    def test_an_invalid_request_never_reaches_github(self):
        self.plan(b"", ["ok"])
        status, output = self.restore(name="../escape")
        self.assertEqual((status, self.calls("list")), (1, []))
        self.assertIn("::error title=Artifact download failed::The requested artifact cannot be requested", output)

    def test_main_passes_only_the_token_and_runner_basics_to_gh(self):
        payload = archive_zip([("backend-tests.tar.zst", ARCHIVE)])
        self.plan(payload, ["ok"])
        environment = dict(self.env, FAKE_PRIVATE="1", GITHUB_REPOSITORY=REPOSITORY, GITHUB_RUN_ID=str(RUN_ID))
        with unittest.mock.patch.dict(os.environ, environment, clear=True), \
                unittest.mock.patch("sys.stdout", io.StringIO()) as out:
            status = restore_module.main(["--name", NAME, "--path", str(self.destination)])
        self.assert_restored(status, out.getvalue())
        self.assertEqual({(call["token"], call["host"], call["leak"]) for call in self.calls("list") + self.calls("zip")},
                         {("synthetic-token", "github.com", False)})

    def test_the_worst_case_restore_leaves_every_consuming_job_most_of_its_timeout(self):
        # Every listing and download attempt runs to its bound, and every wait
        # is the longest a throttled response may ask for.
        waits = 2 * (restore_module.ATTEMPTS - 1) * ci_retry.RETRY_AFTER_CAP
        worst = restore_module.ATTEMPTS * (restore_module.LIST_SECONDS + restore_module.ATTEMPT_SECONDS) + waits
        workflow = yaml.safe_load((ROOT / ".github/workflows/ci-rust.yml").read_text(encoding="utf-8"))
        consumers = {job_id: job["timeout-minutes"] for job_id, job in workflow["jobs"].items()
                     if any(step.get("uses") == "./.github/actions/rust-archive" for step in job.get("steps", []))}
        self.assertEqual(sorted(consumers), ["isolation", "mongo-eg", "mongo-shards", "tests", "timing"])
        for job_id, minutes in consumers.items():
            with self.subTest(job=job_id):
                self.assertLessEqual(worst, minutes * 60 * 4 // 5, "setup and the job's own work need the rest")
        self.assertGreaterEqual(restore_module.ATTEMPT_SECONDS, 300, "an attempt must allow the observed 190 s p100")


if __name__ == "__main__":
    unittest.main()
