"""Regressions for the owner-approved merge-queue Mongo flaky retry.

Proves the approved scope: one retry only for ordinary merge-group shards
through the expiry date, a flag for every pass on retry, and one idempotent
tracking issue written only by the scheduled tracker.
"""
import base64
import datetime
import io
import json
import re
import sys
import tempfile
import unittest
import zipfile
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))
import mongo_flaky_retry as retry  # noqa: E402
import mongo_flaky_tracker as tracker  # noqa: E402

ROOT = Path(__file__).resolve().parents[2]
UNTIL = "2026-12-01"
DAY = datetime.date(2026, 11, 24)
REPOSITORY = "ChronoAIProject/cma"
TEST = "store::sandbox_mongo_integration_tests::lifecycle::reopen_preserves_history"

# The shape nextest 0.9.151 writes with --retries 1, observed on a scratch crate:
# a pass on retry keeps its failed attempts as <flakyFailure>; a test that
# failed every attempt has <failure> plus <rerunFailure>.
JUNIT = f"""<?xml version="1.0" encoding="UTF-8"?>
<testsuites name="nextest-run" tests="3" failures="1" errors="0">
  <testsuite name="cma::bin/cma" tests="3" errors="0" failures="1">
    <testcase name="store::steady" classname="cma::bin/cma" time="0.1"/>
    <testcase name="{TEST}" classname="cma::bin/cma" time="0.1">
      <flakyFailure message="thread &apos;x&apos; panicked at src/a.rs:1:2" type="test failure">thread panicked
first attempt fails &lt;&amp;&gt;
        <system-out>output</system-out>
      </flakyFailure>
    </testcase>
    <testcase name="store::always_fails" classname="cma::bin/cma" time="0.1">
      <failure message="always" type="test failure">always</failure>
      <rerunFailure message="always" type="test failure">always</rerunFailure>
    </testcase>
  </testsuite>
</testsuites>
"""


def outputs(path):
    return dict(line.split("=", 1) for line in Path(path).read_text().splitlines())


class PolicyTests(unittest.TestCase):
    def test_only_ordinary_merge_group_shards_retry_and_only_once(self):
        for shard in retry.ORDINARY_SHARDS:
            self.assertEqual(retry.decide("merge_group", "am", shard, UNTIL, DAY), (1, None))

    def test_no_retries_outside_the_merge_queue(self):
        for event in ("pull_request", "push", "workflow_dispatch", "schedule", "pull_request_target", "", None):
            for shard in retry.ORDINARY_SHARDS:
                self.assertEqual(retry.decide(event, "am", shard, UNTIL, DAY), (0, None), (event, shard))

    def test_isolated_shard_other_tracks_and_unknown_shards_never_retry(self):
        for shard in ("isolated", "0/8", "9/8", "1/9", "", None, "1/8 "):
            self.assertEqual(retry.decide("merge_group", "am", shard, UNTIL, DAY)[0], 0, shard)
        for track in ("eg", "web", "", None):
            self.assertEqual(retry.decide("merge_group", track, "1/8", UNTIL, DAY)[0], 0, track)

    def test_the_last_day_is_inclusive_and_the_next_day_has_zero_retries_with_one_notice(self):
        self.assertEqual(retry.decide("merge_group", "am", "3/8", UNTIL, datetime.date(2026, 12, 1)), (1, None))
        for today in (datetime.date(2026, 12, 2), datetime.date(2027, 1, 1)):
            retries, notice = retry.decide("merge_group", "am", "3/8", UNTIL, today)
            self.assertEqual(retries, 0)
            self.assertTrue(notice.startswith("::notice title=Merge-queue flaky retry expired::"))
            self.assertIn("ended on 2026-12-01", notice)
            self.assertIn(retry.UNTIL_VARIABLE, notice)
            self.assertNotIn("\n", notice)

    def test_a_malformed_expiry_fails_closed(self):
        for until in ("", None, "2026-13-01", "2026-12-1", "2099-12-31 ", "tomorrow", "2026-02-30"):
            retries, warning = retry.decide("merge_group", "am", "1/8", until, DAY)
            self.assertEqual(retries, 0, until)
            self.assertTrue(warning.startswith("::warning title=Merge-queue flaky retry disabled::"))

    def run_policy(self, **overrides):
        with tempfile.TemporaryDirectory() as directory:
            output = Path(directory) / "output"
            env = {"EVENT_NAME": "merge_group", "TRACK": "am", "MONGO_SHARD": "2/8", retry.UNTIL_VARIABLE: UNTIL,
                   "GITHUB_RUN_ATTEMPT": "1", "RUNNER_TEMP": directory, "GITHUB_OUTPUT": str(output)}
            env.update(overrides)
            out = io.StringIO()
            self.assertEqual(retry.policy(env, today=DAY, out=out), 0)
            values = outputs(output)
            if values.get("retries") == "1":
                self.assertTrue(Path(values["report-dir"]).is_dir())
                values["report-dir"] = values["report-dir"].replace(directory, "$RUNNER_TEMP")
            return values, out.getvalue()

    def test_policy_outputs_drive_the_contracts_step(self):
        values, log = self.run_policy()
        self.assertEqual(values, {"retries": "1", "report-dir": "$RUNNER_TEMP/mongo-flaky-retry",
                                  "artifact": "mongo-flaky-retry-am-shard-2-of-8-attempt-1"})
        self.assertIn("active through 2026-12-01", log)
        self.assertEqual(self.run_policy(EVENT_NAME="pull_request")[0], {"retries": "0"})
        self.assertEqual(self.run_policy(MONGO_SHARD="isolated")[0], {"retries": "0"})
        values, log = self.run_policy(**{retry.UNTIL_VARIABLE: "2026-11-23"})
        self.assertEqual(values, {"retries": "0"})
        self.assertEqual(len([line for line in log.splitlines() if line.startswith("::notice")]), 1)

    def test_the_workflow_sets_the_expiry_once_and_the_tracker_reads_the_same_date(self):
        text = (ROOT / ".github/workflows/ci-rust.yml").read_text(encoding="utf-8")
        self.assertEqual(re.findall(rf"{retry.UNTIL_VARIABLE}: \"([^\"]+)\"", text), [UNTIL])
        self.assertEqual(tracker.read_until(text), datetime.date(2026, 12, 1))
        for broken in (text.replace(UNTIL, "2026-12-1"), text + f'      {retry.UNTIL_VARIABLE}: "2027-01-01"\n'):
            with self.assertRaises(ValueError):
                tracker.read_until(broken)


class ReportTests(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory()
        self.root = Path(self.temp.name)
        self.directory = self.root / "mongo-flaky-retry"
        self.directory.mkdir()
        self.env = {"CMA_MONGO_FLAKY_REPORT_DIR": str(self.directory), "MONGO_SHARD": "4/8",
                    "TESTS_OUTCOME": "success", "GITHUB_STEP_SUMMARY": str(self.root / "summary"),
                    "GITHUB_OUTPUT": str(self.root / "output"), "GITHUB_REPOSITORY": REPOSITORY,
                    "GITHUB_RUN_ID": "45314461047", "GITHUB_RUN_ATTEMPT": "2", "EVENT_NAME": "merge_group",
                    "GITHUB_REF": "refs/heads/gh-readonly-queue/develop/pr-1-abc", "GITHUB_SHA": "a" * 40,
                    retry.UNTIL_VARIABLE: UNTIL}

    def tearDown(self):
        self.temp.cleanup()

    def partition(self, stem, junit=JUNIT):
        (self.directory / f"nextest-{stem}.toml").write_text("[profile.mongo-ordinary.junit]\n")
        if junit is not None:
            (self.directory / f"junit-{stem}.xml").write_text(junit)

    def run_report(self, **overrides):
        out = io.StringIO()
        status = retry.report({**self.env, **overrides}, out=out)
        return status, out.getvalue()

    def test_flaky_extraction_yields_one_annotation_summary_row_and_record_per_pass_on_retry(self):
        self.partition("count-1-69")
        self.partition("count-9-69", JUNIT.replace(TEST, TEST + "_second"))
        status, log = self.run_report()
        self.assertEqual(status, 0)
        warnings = [line for line in log.splitlines() if line.startswith("::warning title=Flaky test (retried once)::")]
        self.assertEqual(len(warnings), 2)
        self.assertIn(f"cma::bin/cma {TEST} failed 1 attempt(s) and passed only on its retry in Mongo shard 4/8",
                      warnings[0])
        self.assertNotIn("always_fails", log, "a test that failed every attempt is an ordinary failure")
        self.assertNotIn("store::steady", log)
        summary = Path(self.env["GITHUB_STEP_SUMMARY"]).read_text()
        self.assertIn(f"| `{TEST}` | `cma::bin/cma` | 1 | thread 'x' panicked at src/a.rs:1:2 |", summary)
        self.assertIn("Retry active through 2026-12-01", summary)
        self.assertEqual(outputs(self.env["GITHUB_OUTPUT"]), {"count": "2", "upload": "true"})
        record = json.loads((self.directory / retry.RECORD_FILE).read_text())
        self.assertEqual(record, {
            "schema": retry.RECORD_SCHEMA, "repository": REPOSITORY, "run_id": 45314461047, "run_attempt": 2,
            "event": "merge_group", "ref": self.env["GITHUB_REF"], "sha": "a" * 40, "shard": "4/8",
            "retry_until": UNTIL, "tests": [
                {"binary": "cma::bin/cma", "test": TEST, "failed_attempts": 1},
                {"binary": "cma::bin/cma", "test": TEST + "_second", "failed_attempts": 1}]})

    def test_annotation_data_cannot_break_out_of_its_line(self):
        self.partition("count-1-2", JUNIT.replace(TEST, "a%0Ab&#10;::error::forged"))
        status, log = self.run_report()
        self.assertEqual(status, 0)
        self.assertFalse(any(line.startswith("::error") for line in log.splitlines()))
        self.assertIn("a%250Ab%0A::error::forged", log)

    def test_no_pass_on_retry_writes_no_record_and_skips_the_upload(self):
        self.partition("count-1-2", JUNIT.replace("flakyFailure", "skipped"))
        status, log = self.run_report()
        self.assertEqual(status, 0)
        self.assertNotIn("::warning", log)
        self.assertIn("No test needed its retry.", Path(self.env["GITHUB_STEP_SUMMARY"]).read_text())
        self.assertEqual(outputs(self.env["GITHUB_OUTPUT"]), {"count": "0", "upload": "false"})
        self.assertFalse((self.directory / retry.RECORD_FILE).exists())

    def test_a_retried_partition_without_its_report_fails_a_passing_shard(self):
        self.partition("count-1-2")
        self.partition("count-2-2", junit=None)
        status, log = self.run_report()
        self.assertEqual(status, 1)
        self.assertIn("::error title=Flaky retry report missing::", log)
        # A failed contracts step already fails the job; its flaky tests are still flagged.
        status, log = self.run_report(TESTS_OUTCOME="failure")
        self.assertEqual(status, 0)
        self.assertEqual(log.count("::warning title=Flaky test (retried once)::"), 1)

    def test_an_unreadable_report_cannot_hide_a_pass_on_retry(self):
        self.partition("count-1-2", "<testsuites><testsuite>")
        self.assertEqual(self.run_report()[0], 1)
        self.partition("count-1-2", "<other/>")
        self.assertEqual(self.run_report()[0], 1)

    def test_a_policy_without_any_retried_partition_warns(self):
        status, log = self.run_report()
        self.assertEqual(status, 0)
        self.assertIn("::warning title=Flaky retry not applied::", log)


class FakeGitHub:
    repository = REPOSITORY

    def __init__(self, issues=(), artifacts=(), runs=None, archives=None):
        self.issues = [dict(issue) for issue in issues]
        self.artifacts, self.runs, self.archives = list(artifacts), runs or {}, archives or {}
        self.writes, self.reads = [], []

    def get(self, path):
        self.reads.append(path)
        if path.startswith("issues?"):
            assert "creator=github-actions%5Bbot%5D" in path and "state=all" in path
            page = int(re.search(r"&page=([0-9]+)", path).group(1))
            return self.issues[(page - 1) * 100:page * 100]
        if path.startswith("actions/artifacts?name="):
            name = path.split("=", 1)[1].split("&", 1)[0]
            return {"artifacts": [row for row in self.artifacts if row["name"] == name]}
        if path.startswith("actions/runs/"):
            return self.runs[int(path.rsplit("/", 1)[1])]
        raise AssertionError(path)

    def get_bytes(self, path):
        self.reads.append(path)
        return self.archives[int(path.split("/")[2])]

    def send(self, method, path, payload):
        self.writes.append((method, path, payload))
        if (method, path) == ("POST", "issues"):
            issue = {"number": 900 + len(self.issues), "state": "open", **payload}
            self.issues.append(issue)
            return issue
        if method == "PATCH":
            issue = next(row for row in self.issues if f"issues/{row['number']}" == path)
            issue.update(payload)
            return issue
        return {}


def run_fixture(identifier, attempt=1, event="merge_group", branch="gh-readonly-queue/develop/pr-7-abc",
                repository=REPOSITORY):
    return {"id": identifier, "event": event, "path": ".github/workflows/ci.yml", "head_branch": branch,
            "repository": {"full_name": REPOSITORY}, "head_repository": {"full_name": repository},
            "html_url": f"https://github.com/{REPOSITORY}/actions/runs/{identifier}", "run_attempt": attempt}


def archive(record):
    data = io.BytesIO()
    with zipfile.ZipFile(data, "w") as bundle:
        bundle.writestr(retry.RECORD_FILE, json.dumps(record))
    return data.getvalue()


def queued(fake, artifact_id, run_id, shard="3/8", attempt=1, tests=None, created="2026-11-24T05:00:00Z", **run):
    record = {"schema": retry.RECORD_SCHEMA, "repository": REPOSITORY, "run_id": run_id, "run_attempt": attempt,
              "event": "merge_group", "ref": "refs/heads/gh-readonly-queue/develop/pr-7-abc", "sha": "b" * 40,
              "shard": shard, "retry_until": UNTIL,
              "tests": tests or [{"binary": "cma::bin/cma", "test": TEST, "failed_attempts": 1}]}
    payload = archive(record)
    fake.artifacts.append({"id": artifact_id, "name": retry.artifact_name(shard, attempt), "expired": False,
                           "size_in_bytes": len(payload), "created_at": created,
                           "workflow_run": {"id": run_id, "repository_id": 1, "head_repository_id": 1}})
    fake.runs[run_id] = run_fixture(run_id, attempt, **run)
    fake.archives[artifact_id] = payload
    return record


class TrackerTests(unittest.TestCase):
    def track(self, fake, today=DAY):
        out = io.StringIO()
        self.assertEqual(tracker.track(fake, datetime.date(2026, 12, 1), today, out=out), 0)
        return out.getvalue()

    def test_nothing_retried_opens_no_issue(self):
        fake = FakeGitHub()
        self.assertIn("No test has passed only on its merge-queue retry.", self.track(fake))
        self.assertEqual(fake.writes, [])
        names = {path.split("=", 1)[1].split("&", 1)[0] for path in fake.reads if path.startswith("actions/artifacts")}
        self.assertEqual(names, {retry.artifact_name(shard, attempt) for shard in retry.ORDINARY_SHARDS
                                 for attempt in range(1, retry.MAX_ATTEMPT + 1)})

    def test_one_issue_is_opened_with_counts_run_links_labels_and_the_expiry(self):
        fake = FakeGitHub()
        queued(fake, 11, 501, shard="3/8")
        queued(fake, 12, 502, shard="5/8", attempt=2, created="2026-11-24T06:00:00Z")
        self.track(fake)
        self.assertEqual([(method, path) for method, path, _ in fake.writes], [("POST", "issues")])
        payload = fake.writes[0][2]
        self.assertEqual(payload["title"], "Flaky tests retried in the merge queue")
        self.assertEqual(payload["labels"], ["go-live-should-fix", "priority:P2", "track:infra-ci-release"])
        body = payload["body"]
        self.assertTrue(body.startswith(tracker.MARKER))
        self.assertIn(f"| `{TEST}` (`cma::bin/cma`) | 2 | 3/8, 5/8 | 2026-11-24 05:00 | 2026-11-24 06:00 |", body)
        self.assertIn(f"[502#2](https://github.com/{REPOSITORY}/actions/runs/502/attempts/2), "
                      f"[501#1](https://github.com/{REPOSITORY}/actions/runs/501/attempts/1)", body)
        self.assertIn("active through **2026-12-01** (UTC, inclusive)", body)
        self.assertIn("CMA_MONGO_FLAKY_RETRY_UNTIL", body)

    def test_rerunning_is_idempotent_and_never_duplicates_the_issue(self):
        fake = FakeGitHub()
        queued(fake, 11, 501)
        self.track(fake)
        self.track(fake)
        self.track(fake)
        self.assertEqual([(method, path) for method, path, _ in fake.writes], [("POST", "issues")])
        self.assertEqual(len(fake.issues), 1)
        state = tracker.parse_state(fake.issues[0]["body"])
        self.assertEqual(state["artifacts"], [11])
        self.assertEqual(state["tests"][f"cma::bin/cma {TEST}"]["count"], 1)
        # Archives of counted artifacts are never downloaded again.
        self.assertEqual(fake.reads.count("actions/artifacts/11/zip"), 1)

    def test_a_new_occurrence_updates_the_same_issue_and_comments_once(self):
        fake = FakeGitHub()
        queued(fake, 11, 501)
        self.track(fake)
        queued(fake, 12, 502, shard="8/8", created="2026-11-24T07:00:00Z")
        self.track(fake)
        self.track(fake)
        self.assertEqual([(method, path) for method, path, _ in fake.writes],
                         [("POST", "issues"), ("PATCH", "issues/900"), ("POST", "issues/900/comments")])
        self.assertIn("| 2 | 3/8, 8/8 |", fake.issues[0]["body"])
        self.assertIn("Mongo shard 8/8, [run 502 attempt 1]", fake.writes[2][2]["body"])

    def test_a_closed_issue_reopens_only_for_a_new_retried_pass(self):
        fake = FakeGitHub()
        queued(fake, 11, 501)
        self.track(fake)
        fake.issues[0]["state"] = "closed"
        self.track(fake)
        self.assertEqual(len(fake.writes), 1, "no new record leaves a closed issue closed")
        queued(fake, 12, 502, created="2026-11-25T01:00:00Z")
        self.track(fake)
        self.assertEqual(fake.writes[1][0:2], ("PATCH", "issues/900"))
        self.assertEqual(fake.writes[1][2]["state"], "open")
        self.assertEqual(len(fake.issues), 1)

    def test_only_issues_with_the_marker_and_exact_title_are_the_tracking_issue(self):
        others = [{"number": 5, "state": "open", "title": "Flaky tests retried in the merge queue", "body": "human"},
                  {"number": 6, "state": "open", "title": "Other", "body": tracker.MARKER},
                  {"number": 7, "state": "open", "title": "PR", "body": tracker.MARKER, "pull_request": {}}]
        fake = FakeGitHub(issues=others)
        queued(fake, 11, 501)
        self.track(fake)
        self.assertEqual(fake.writes[0][0:2], ("POST", "issues"))

    def test_records_from_outside_a_merge_group_ci_run_of_this_repository_are_refused(self):
        for run in ({"event": "pull_request"}, {"branch": "feature/gh-readonly-queue/develop/x"},
                    {"repository": "fork/cma"}, {"event": "push", "branch": "develop"}):
            fake = FakeGitHub()
            queued(fake, 11, 501, **run)
            log = self.track(fake)
            self.assertIn("::warning title=Flaky record skipped::", log)
            self.assertEqual(fake.writes, [], run)
            self.assertNotIn("actions/artifacts/11/zip", fake.reads, "nothing is downloaded from another run")

    def test_a_record_must_match_its_artifact_and_name_plain_tests(self):
        cases = [{"tests": [{"binary": "cma::bin/cma", "test": "x` | <img src=y>", "failed_attempts": 1}]},
                 {"tests": [{"binary": "cma::bin/cma", "test": TEST, "failed_attempts": 0}]},
                 {"shard": "4/8"}, {"run_id": 999}, {"run_attempt": 2}, {"schema": "other"}, {"event": "push"}]
        for change in cases:
            fake = FakeGitHub()
            record = queued(fake, 11, 501)
            record.update(change)
            fake.archives[11] = archive(record)
            fake.artifacts[0]["size_in_bytes"] = len(fake.archives[11])
            self.assertIn("::warning title=Flaky record skipped::", self.track(fake), change)
            self.assertEqual(fake.writes, [], change)

    def test_oversized_expired_or_unexpected_archives_are_not_read(self):
        fake = FakeGitHub()
        queued(fake, 11, 501)
        fake.artifacts[0]["size_in_bytes"] = tracker.MAX_RECORD_BYTES + 1
        self.track(fake)
        fake.artifacts[0].update(size_in_bytes=len(fake.archives[11]), expired=True)
        self.track(fake)
        self.assertEqual(fake.writes, [])
        self.assertNotIn("actions/artifacts/11/zip", fake.reads)
        data = io.BytesIO()
        with zipfile.ZipFile(data, "w") as bundle:
            bundle.writestr(retry.RECORD_FILE, "{}")
            bundle.writestr("extra.json", "{}")
        fake.archives[11] = data.getvalue()
        fake.artifacts[0].update(expired=False, size_in_bytes=len(fake.archives[11]))
        self.assertIn("must hold exactly the flaky record", self.track(fake))
        self.assertEqual(fake.writes, [])

    def test_the_window_line_reports_the_expiry_and_the_collector_stops_after_retention(self):
        fake = FakeGitHub()
        queued(fake, 11, 501)
        self.track(fake)
        self.track(fake, today=datetime.date(2026, 12, 2))
        self.assertEqual(fake.writes[-1][0:2], ("PATCH", "issues/900"))
        self.assertIn("**ended on 2026-12-01** (UTC); the shards run without retries again", fake.issues[0]["body"])
        writes, reads = len(fake.writes), len(fake.reads)
        self.track(fake, today=datetime.date(2026, 12, 2))
        self.assertEqual(len(fake.writes), writes, "an unchanged body is never rewritten")
        self.assertIn("nothing to collect", self.track(fake, today=datetime.date(2026, 12, 6)))
        self.assertEqual(len(fake.reads), reads + 1 + 8 * retry.MAX_ATTEMPT)

    def test_state_round_trips_and_a_tampered_state_fails_loudly(self):
        state = {"artifacts": [3], "tests": {}}
        body = tracker.render(state, datetime.date(2026, 12, 1), DAY)
        self.assertEqual(tracker.parse_state(body), state)
        self.assertEqual(tracker.parse_state("no state here"), tracker.empty_state())
        broken = base64.b64encode(b'{"artifacts": 3}').decode()
        with self.assertRaises(ValueError):
            tracker.parse_state(f"<!-- cma-mongo-flaky-state:{broken} -->")

    def test_github_calls_are_bounded_and_carry_no_shell(self):
        calls = []

        class Result:
            returncode, stdout, stderr = 0, b'{"ok": true}', b""

        def run(command, **options):
            calls.append((command, options))
            return Result()

        api = tracker.GitHub(REPOSITORY, run=run, timeout=7)
        api.get("actions/runs/1")
        api.send("PATCH", "issues/2", {"body": "x"})
        self.assertEqual(calls[0][0], ["gh", "api", f"repos/{REPOSITORY}/actions/runs/1"])
        self.assertEqual(calls[1][0], ["gh", "api", "-X", "PATCH", f"repos/{REPOSITORY}/issues/2", "--input", "-"])
        self.assertEqual(json.loads(calls[1][1]["input"]), {"body": "x"})
        self.assertTrue(all(options["timeout"] == 7 and "shell" not in options for _, options in calls))


if __name__ == "__main__":
    unittest.main()
