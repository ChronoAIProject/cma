"""A develop build proves its layer index was saved, or names why later builds will be cold."""
import contextlib
import datetime
import io
import json
import os
import subprocess
import tempfile
import unittest
from pathlib import Path
from unittest.mock import Mock, patch

import staging_cd_cache_save as cache_save
from staging_cd_common import Refusal

STARTED = datetime.datetime(2026, 11, 21, 15, 40, tzinfo=datetime.timezone.utc).timestamp()
DEVELOP = {"GITHUB_REPOSITORY": "ChronoAIProject/cma", "GITHUB_EVENT_NAME": "push",
           "GITHUB_REF": "refs/heads/develop", "GH_TOKEN": "synthetic-token"}
PRIVATE = "private-marker"
LAYER_WARNING = ("::warning title=Build cache not saved::runtime build layers were not saved; the repository's "
                 "Actions cache may be read-only (budget reached) or the export failed. "
                 "Later runtime builds will start cold and run slower.")


def entry(key, seconds=0, ref="refs/heads/develop"):
    created = datetime.datetime.fromtimestamp(STARTED + seconds, datetime.timezone.utc)
    return {"id": 1, "key": key, "ref": ref, "version": "c" * 64, "size_in_bytes": 47393,
            "created_at": created.isoformat().replace("+00:00", "Z"),
            "last_accessed_at": created.isoformat().replace("+00:00", "Z")}


class Lists:
    """A cache list API answering each read in turn with rows, or an exception to raise."""

    def __init__(self, *answers):
        self.answers, self.paths, self.deadlines = list(answers), [], []

    def request(self, path, *, deadline=None):
        self.paths.append(path)
        self.deadlines.append(deadline)
        answer = self.answers.pop(0)
        if isinstance(answer, BaseException):
            raise answer
        return answer if isinstance(answer, dict) else {"total_count": len(answer), "actions_caches": answer}


def gh_response(status, rows):
    """What `gh api --include` prints: the status line, headers, a blank line and the body."""
    return ("HTTP/2.0 " + status + "\r\nContent-Type: application/json; charset=utf-8\r\n"
            "X-Github-Api-Version-Selected: 2022-11-28\r\n\r\n"
            + json.dumps({"total_count": len(rows), "actions_caches": rows}))


class Clock:
    def __init__(self):
        self.now = 1000.0

    def __call__(self):
        return self.now

    def advance(self, seconds):
        self.now += seconds


class LayerTests(unittest.TestCase):
    def check(self, api, *, summary=None, env=None, component="runtime", scope="cma-runtime"):
        stream = io.StringIO()
        self.assertIsNone(cache_save.report_layers(component, scope, summary or {}, STARTED, env or DEVELOP,
                                                   stream=stream, api=api))
        return stream.getvalue().splitlines()

    def test_a_new_index_entry_of_this_scope_and_writer_is_saved(self):
        api = Lists([entry("index-cma-runtime-1-fd3dfe2c#71", seconds=300)])
        self.assertEqual(self.check(api), ["CACHE-SAVE cache=build-layers component=runtime result=saved"])
        self.assertEqual(api.paths, ["actions/caches?key=index-cma-runtime-&ref=refs/heads/develop"
                                     "&sort=created_at&direction=desc&per_page=100"])
        # The single read carries a short absolute bound.
        self.assertIsInstance(api.deadlines[0], float)
        # A runner clock slightly ahead of the service still proves the save.
        self.assertEqual(self.check(Lists([entry("index-cma-runtime-1-fd3dfe2c#71", seconds=-30)]))[0],
                         "CACHE-SAVE cache=build-layers component=runtime result=saved")

    def test_older_entries_other_scopes_writers_and_refs_do_not_prove_a_save(self):
        rows = [entry("index-cma-runtime-omarchy-1-fd3dfe2c#40", seconds=300),
                entry("index-cma-runtime-1-0123abcd#3", seconds=300),
                entry("index-cma-runtime-1-fd3dfe2c#99", seconds=300, ref="refs/pull/12/merge"),
                entry("index-cma-runtime-1-fd3dfe2c", seconds=300),
                entry("index-cma-runtime-1-fd3dfe2c#70", seconds=-3300)]
        api = Lists(rows)
        self.assertEqual(self.check(api), ["CACHE-SAVE cache=build-layers component=runtime result=not-saved "
                                           "reason=absent", LAYER_WARNING])
        # The check runs minutes after the export, so one read settles absence.
        self.assertEqual(len(api.paths), 1)

    def test_buildkit_export_errors_name_the_cause_without_reading_the_cache_list(self):
        for error, reason, cause in (
                ("read-only", "read-only", "Actions cache is read-only: organization budget reached."),
                ("failed", "export-failed", "BuildKit reported that their cache export failed")):
            with self.subTest(error=error):
                api = Mock()
                lines = self.check(api, summary={"cache_export_error": error}, component="web", scope="cma-web")
                api.request.assert_not_called()
                self.assertEqual(lines[0], f"CACHE-SAVE cache=build-layers component=web result=not-saved reason={reason}")
                self.assertTrue(lines[1].startswith("::warning title=Build cache not saved::web build layers were not saved"))
                self.assertIn(cause, lines[1])
                self.assertIn("Later web builds will start cold and run slower", lines[1])
                self.assertEqual(len(lines), 2)

    def test_unreadable_or_invalid_cache_lists_are_unknown_and_never_warn(self):
        failure = Refusal("GitHub API request failed")
        for answer in (failure, {"actions_caches": PRIVATE}, {}, {"actions_caches": [{"key": 1}]},
                       {"actions_caches": [dict(entry("index-cma-runtime-1-fd3dfe2c#71"), created_at=PRIVATE)]}):
            with self.subTest(answer=answer):
                api = Lists(answer)
                self.assertEqual(self.check(api), ["CACHE-SAVE cache=build-layers component=runtime "
                                                   "result=unknown reason=unreadable"])
                self.assertEqual(len(api.paths), 1)
        # A missing credential is also unknown, and the check never raises.
        self.assertEqual(self.check(None, env={k: v for k, v in DEVELOP.items() if k != "GH_TOKEN"}),
                         ["CACHE-SAVE cache=build-layers component=runtime result=unknown reason=unreadable"])
        self.assertEqual(self.check(Lists([entry("index-cma-runtime-1-fd3dfe2c#71")]), summary=PRIVATE)[0],
                         "CACHE-SAVE cache=build-layers component=runtime result=saved")

    def test_pull_request_and_other_builds_expect_no_export_and_print_nothing(self):
        for changes in ({"GITHUB_EVENT_NAME": "pull_request", "GITHUB_REF": "refs/pull/12/merge"},
                        {"GITHUB_REF": "refs/heads/main"}, {"GITHUB_REPOSITORY": "example/fork"}):
            with self.subTest(changes=changes):
                api = Mock()
                self.assertEqual(self.check(api, env=dict(DEVELOP, **changes)), [])
                api.request.assert_not_called()
        self.assertEqual(self.check(Mock(), component=PRIVATE), [])

    def test_legacy_remote_layer_exports_are_disabled(self):
        from staging_cd_build_cache import BuildCache
        with tempfile.TemporaryDirectory() as directory:
            base = dict(DEVELOP, GITHUB_SHA="a" * 40, GITHUB_RUN_ID="1", GITHUB_RUN_ATTEMPT="1",
                        CD_COMPONENT="runtime", RUNNER_TEMP=directory)
            runtime = {"ACTIONS_RUNTIME_TOKEN": "synthetic-runtime", "ACTIONS_RESULTS_URL": "https://example.invalid"}
            root = Path(directory)
            self.assertIs(BuildCache(root, dict(base, **runtime), root, "runtime").exports_layers(), False)
            self.assertIs(BuildCache(root, base, root, "runtime").exports_layers(), False)
            cache = BuildCache(root, dict(base, **runtime), root, "runtime")
            cache.env = dict(cache.env, GITHUB_EVENT_NAME="pull_request", GITHUB_REF="refs/pull/12/merge")
            self.assertIs(cache.exports_layers(), False)


class GitHubReadTests(unittest.TestCase):
    """The check reads through the real GitHub helper, inside one short bound."""

    def check(self, api, clock=None):
        stream = io.StringIO()
        options = {} if clock is None else {"clock": clock}
        cache_save.report_layers("runtime", "cma-runtime", {}, STARTED, DEVELOP, stream=stream, api=api, **options)
        return stream.getvalue().splitlines()

    def test_a_gh_api_include_response_reaches_a_saved_or_absent_result(self):
        # A drifted request signature or response parser would turn every
        # result into unknown; this reads a canned response end to end.
        from staging_cd_github import GitHub
        for rows, outcome in (([entry("index-cma-runtime-1-fd3dfe2c#71", seconds=300)], "result=saved"),
                              ([entry("index-cma-runtime-1-fd3dfe2c#70", seconds=-3300)], "result=not-saved reason=absent")):
            with self.subTest(outcome=outcome):
                runner = Mock(return_value=None)
                runner.side_effect = lambda args, rows=rows, **_kwargs: subprocess.CompletedProcess(
                    args, 0, stdout=gh_response("200 OK", rows), stderr="")
                lines = self.check(GitHub("synthetic-token", runner=runner))
                self.assertEqual(lines[0], "CACHE-SAVE cache=build-layers component=runtime " + outcome)
                runner.assert_called_once()
                args, kwargs = runner.call_args.args[0], runner.call_args.kwargs
                self.assertEqual(args[-1], "repos/ChronoAIProject/cma/actions/caches?key=index-cma-runtime-"
                                           "&ref=refs/heads/develop&sort=created_at&direction=desc&per_page=100")
                self.assertIn("--include", args)
                self.assertEqual(args[args.index("--method") + 1], "GET")
                self.assertEqual(kwargs["env"]["GH_TOKEN"], "synthetic-token")
                self.assertLessEqual(kwargs["timeout"], cache_save.READ_SECONDS)

    def test_the_worst_case_is_one_bounded_read_and_no_wait(self):
        from staging_cd_github import GitHub
        # Well under any step limit; the publish job allows hours.
        self.assertLessEqual(cache_save.READ_SECONDS, 30)

        def hang(args, *, timeout, **_kwargs):
            clock.advance(timeout)
            raise subprocess.TimeoutExpired(args, timeout)

        def unavailable(args, **_kwargs):
            clock.advance(0.5)
            return subprocess.CompletedProcess(args, 1, stdout=gh_response("503 Service Unavailable", []),
                                               stderr="gh: Service Unavailable (HTTP 503)")
        for behaviour in (hang, unavailable):
            with self.subTest(behaviour=behaviour.__name__), patch("time.sleep") as sleep:
                clock = Clock()
                runner = Mock(side_effect=behaviour)
                api = GitHub("synthetic-token", runner=runner,
                             retry={"clock": clock, "sleep": clock.advance, "rand": lambda: 1.0,
                                    "log": lambda _line: None})
                self.assertEqual(self.check(api, clock),
                                 ["CACHE-SAVE cache=build-layers component=runtime result=unknown reason=unreadable"])
                # One read: its repeats share one bound, and nothing else waits.
                self.assertLessEqual(clock.now - 1000.0, cache_save.READ_SECONDS)
                self.assertLessEqual(runner.call_count, 3)
                sleep.assert_not_called()


class PublicationTests(unittest.TestCase):
    """The publish job checks the layer index of its last export, and only diagnoses."""

    def publish(self, exports, *, summary=None, check=None):
        import staging_cd_publish as publication
        from staging_cd_receipts_tests import fixture_identity, fixture_receipt
        identity = fixture_identity()
        reservation = {k: v for k, v in identity.items() if k != "build"}
        reservation["schema"] = "cma-version-reservation-v1"
        with tempfile.TemporaryDirectory() as directory, contextlib.ExitStack() as stack:
            env = dict(DEVELOP, GITHUB_RUN_ID="100", GITHUB_RUN_ATTEMPT="1", RUNNER_TEMP=directory,
                       CD_COMPONENT="web", CD_BUILD_CACHE="enabled", GITHUB_OUTPUT=str(Path(directory) / "output"),
                       CD_RECEIPTS_PATH=str(Path(directory) / "receipts.json"),
                       CD_TASKS=json.dumps([{"component": "web", "mode": "publish", "reservation": reservation}]))
            stack.enter_context(patch.dict(os.environ, env, clear=True))
            for name in ("event_identity", "Registry", "GitHub", "Reservations", "snapshot"):
                stack.enter_context(patch.object(publication, name))
            cache = Mock(restore_result="miss", export_reason=None, export_headroom=None)
            cache.exports_layers.return_value = exports
            cache.export_mounts.return_value = False
            stack.enter_context(patch.object(publication, "BuildCache", return_value=cache))
            stack.enter_context(patch.object(publication, "identity", return_value=(identity, {}, {})))
            stack.enter_context(patch.object(publication, "try_reuse", return_value=None))

            def build(*_args, progress, **_kwargs):
                progress.update(summary or {"complete": True, "vertices": 1, "cached_vertices": 0})
                return Path(directory) / "image.tar", identity
            stack.enter_context(patch.object(publication, "build", side_effect=build))
            stack.enter_context(patch.object(publication, "publish", return_value=fixture_receipt()))
            # The script passes its live environment; keep what it held at the call.
            layers = stack.enter_context(patch.object(publication, "report_layers"))
            layers.side_effect = check or (lambda *args: layers.snapshots.append(args[:4] + (dict(args[4]),)))
            layers.snapshots = []
            output = stack.enter_context(contextlib.redirect_stdout(io.StringIO()))
            before = datetime.datetime.now(datetime.timezone.utc).timestamp()
            status = publication.main()
            return status, layers, before, output.getvalue()

    def test_a_develop_export_is_verified_for_its_own_scope_after_publication(self):
        summary = {"complete": True, "vertices": 1, "cached_vertices": 0, "cache_export_error": "read-only"}
        status, layers, before, output = self.publish(True, summary=summary)
        self.assertEqual(status, 0)
        layers.assert_called_once()
        [(component, scope, observed, started, env)] = layers.snapshots
        self.assertEqual((component, scope, observed), ("web", "cma-cd-web", summary))
        self.assertGreaterEqual(started, before - 1)
        self.assertEqual(env["CD_COMPONENT"], "web")
        # The ignored export failure no longer reads as a failed Dockerfile step.
        self.assertNotIn("Image build step failed", output)

    def test_a_failing_check_prints_unknown_and_never_changes_the_publication_outcome(self):
        def broken(*_args):
            raise RuntimeError(PRIVATE)
        status, layers, _, output = self.publish(True, check=broken)
        self.assertEqual(status, 0)
        layers.assert_called_once()
        self.assertIn("CACHE-SAVE cache=build-layers component=web result=unknown reason=unreadable\n", output)
        self.assertNotIn(PRIVATE, output)
        self.assertNotIn("::warning", output)

    def test_a_build_without_a_layer_export_expects_no_save(self):
        for exports in (False, Mock()):
            with self.subTest(exports=exports):
                status, layers, _, _ = self.publish(exports)
                self.assertEqual(status, 0)
                layers.assert_not_called()


if __name__ == "__main__":
    unittest.main()
