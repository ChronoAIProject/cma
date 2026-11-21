"""Each cache save point proves its entry exists, or names why later runs will be cold."""
import contextlib
import datetime
import io
import json
import os
import tempfile
import unittest
from pathlib import Path
from unittest.mock import Mock, patch

import staging_cd_cache_save as cache_save
from staging_cd_common import Refusal

STARTED = datetime.datetime(2026, 11, 21, 15, 40, tzinfo=datetime.timezone.utc).timestamp()
DEVELOP = {"GITHUB_REPOSITORY": "ChronoAIProject/cma", "GITHUB_EVENT_NAME": "push",
           "GITHUB_REF": "refs/heads/develop", "GH_TOKEN": "synthetic-token"}
KEY = "cma-develop-selection-v2-" + "a" * 64 + "-" + "b" * 40 + "-100-1"
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


class ObserveTests(unittest.TestCase):
    """Absence counts only after the last bounded read; an unreadable list proves nothing."""

    def observe(self, *answers):
        sleeps, rows = [], list(answers)

        def read():
            answer = rows.pop(0)
            if isinstance(answer, BaseException):
                raise answer
            return answer
        result = cache_save.observe(read, lambda row: row == "present", sleep=sleeps.append)
        return result, sleeps

    def test_an_entry_seen_on_any_read_is_saved_without_further_waiting(self):
        self.assertEqual(self.observe(["present"]), ("saved", []))
        self.assertEqual(self.observe([], ["other", "present"]), ("saved", [cache_save.WAITS[0]]))

    def test_absence_is_concluded_only_after_every_bounded_wait(self):
        result, sleeps = self.observe([], [], [], [])
        self.assertEqual(result, "not-saved")
        self.assertEqual(sleeps, list(cache_save.WAITS))
        self.assertLessEqual(sum(sleeps), 30)

    def test_api_errors_are_unknown_never_not_saved(self):
        failure = Refusal("GitHub API request failed")
        self.assertEqual(self.observe(failure, failure, failure, failure)[0], "unknown")
        # An early absence may be eventual consistency; only the last read decides.
        self.assertEqual(self.observe([], [], [], failure)[0], "unknown")
        self.assertEqual(self.observe(failure, failure, failure, [])[0], "not-saved")
        self.assertEqual(self.observe(failure, ["present"])[0], "saved")


class LayerTests(unittest.TestCase):
    def check(self, api, *, summary=None, env=None, component="runtime", scope="cma-runtime"):
        stream = io.StringIO()
        self.assertIsNone(cache_save.report_layers(component, scope, summary or {}, STARTED, env or DEVELOP,
                                                   stream=stream, api=api, sleep=lambda _seconds: None))
        return stream.getvalue().splitlines()

    def test_a_new_index_entry_of_this_scope_and_writer_is_saved(self):
        api = Lists([entry("index-cma-runtime-1-fd3dfe2c#71", seconds=300)])
        self.assertEqual(self.check(api), ["CACHE-SAVE cache=build-layers component=runtime result=saved"])
        self.assertEqual(api.paths, ["actions/caches?key=index-cma-runtime-&ref=refs/heads/develop"
                                     "&sort=created_at&direction=desc&per_page=100"])
        # Each read has its own short absolute bound.
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
        api = Lists(rows, rows, rows, rows)
        self.assertEqual(self.check(api), ["CACHE-SAVE cache=build-layers component=runtime result=not-saved "
                                           "reason=absent", LAYER_WARNING])
        self.assertEqual(len(api.paths), 1 + len(cache_save.WAITS))

    def test_buildkit_export_errors_name_the_cause_without_waiting_for_the_api(self):
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
        for api in (Lists(failure, failure, failure, failure),
                    Lists({"actions_caches": PRIVATE}, {}, {"actions_caches": [{"key": 1}]},
                          {"actions_caches": [dict(entry("index-cma-runtime-1-fd3dfe2c#71"), created_at=PRIVATE)]})):
            with self.subTest(api=api):
                self.assertEqual(self.check(api), ["CACHE-SAVE cache=build-layers component=runtime "
                                                   "result=unknown reason=unreadable"])
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

    def test_only_a_develop_push_with_the_cache_runtime_exports_layers(self):
        from staging_cd_build_cache import BuildCache
        with tempfile.TemporaryDirectory() as directory:
            base = dict(DEVELOP, GITHUB_SHA="a" * 40, GITHUB_RUN_ID="1", GITHUB_RUN_ATTEMPT="1",
                        CD_COMPONENT="runtime", RUNNER_TEMP=directory)
            runtime = {"ACTIONS_RUNTIME_TOKEN": "synthetic-runtime", "ACTIONS_RESULTS_URL": "https://example.invalid"}
            root = Path(directory)
            self.assertIs(BuildCache(root, dict(base, **runtime), root, "runtime").exports_layers(), True)
            self.assertIs(BuildCache(root, base, root, "runtime").exports_layers(), False)
            cache = BuildCache(root, dict(base, **runtime), root, "runtime")
            cache.env = dict(cache.env, GITHUB_EVENT_NAME="pull_request", GITHUB_REF="refs/pull/12/merge")
            self.assertIs(cache.exports_layers(), False)


class SelectionTests(unittest.TestCase):
    def check(self, api, env=None):
        stream = io.StringIO()
        with patch.object(cache_save, "GitHub", return_value=api):
            status = cache_save.report_selection(dict(DEVELOP, CD_CACHE_KEY=KEY) if env is None else env,
                                                 stream=stream, sleep=lambda _seconds: None)
        self.assertEqual(status, 0)
        return stream.getvalue().splitlines()

    def test_the_exact_attempt_key_proves_the_save(self):
        api = Lists([entry(KEY + "0"), entry(KEY)])
        self.assertEqual(self.check(api), ["CACHE-SAVE cache=selection result=saved"])
        self.assertEqual(api.paths, ["actions/caches?key=" + KEY + "&ref=refs/heads/develop"
                                     "&sort=created_at&direction=desc&per_page=100"])

    def test_a_missing_key_names_the_slower_next_plan_and_the_likely_cause(self):
        rows = [entry(KEY + "0"), entry(KEY, ref="refs/pull/12/merge")]
        lines = self.check(Lists(rows, rows, rows, rows))
        self.assertEqual(lines, [
            "CACHE-SAVE cache=selection result=not-saved reason=absent",
            "::warning title=Selection cache not saved::This plan's analysed revisions were not saved; the "
            "repository's Actions cache may be read-only (budget reached) or the save failed. The next plan "
            "will re-analyse these revisions and run slower."])

    def test_errors_and_invalid_inputs_are_unknown_and_never_fail_the_step(self):
        failure = Refusal("GitHub API request failed")
        self.assertEqual(self.check(Lists(failure, failure, failure, failure)),
                         ["CACHE-SAVE cache=selection result=unknown reason=unreadable"])
        for key in (None, "", "bad key", "a" * 513, PRIVATE + "&ref=refs/heads/main"):
            with self.subTest(key=key):
                api = Mock()
                env = dict(DEVELOP, CD_CACHE_KEY=key) if key is not None else dict(DEVELOP)
                self.assertEqual(self.check(api, env), ["CACHE-SAVE cache=selection result=unknown reason=unreadable"])
                api.request.assert_not_called()

    def test_only_a_develop_push_expects_a_selection_save(self):
        api = Mock()
        self.assertEqual(self.check(api, dict(DEVELOP, CD_CACHE_KEY=KEY, GITHUB_EVENT_NAME="pull_request",
                                              GITHUB_REF="refs/pull/12/merge")), [])
        api.request.assert_not_called()

    def test_entrypoint_verifies_the_selection_and_exits_zero(self):
        stream = io.StringIO()
        seen = []
        with patch.dict(os.environ, dict(DEVELOP, CD_CACHE_KEY=KEY), clear=True), \
                patch.object(cache_save, "report_selection", side_effect=lambda env: seen.append(dict(env)) or 0), \
                contextlib.redirect_stdout(stream):
            self.assertEqual(cache_save.main(["selection"]), 0)
        self.assertEqual([env["CD_CACHE_KEY"] for env in seen], [KEY])
        self.assertEqual(cache_save.main(["unknown"]), 2)


class PublicationTests(unittest.TestCase):
    """The publish job checks the layer index of its last export, and only diagnoses."""

    def publish(self, exports, *, summary=None):
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
            layers.side_effect = lambda *args: layers.snapshots.append(args[:4] + (dict(args[4]),))
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
        self.assertEqual((component, scope, observed), ("web", "cma-web", summary))
        self.assertGreaterEqual(started, before - 1)
        self.assertEqual(env["CD_COMPONENT"], "web")
        # The ignored export failure no longer reads as a failed Dockerfile step.
        self.assertNotIn("Image build step failed", output)

    def test_a_build_without_a_layer_export_expects_no_save(self):
        for exports in (False, Mock()):
            with self.subTest(exports=exports):
                status, layers, _, _ = self.publish(exports)
                self.assertEqual(status, 0)
                layers.assert_not_called()


if __name__ == "__main__":
    unittest.main()
