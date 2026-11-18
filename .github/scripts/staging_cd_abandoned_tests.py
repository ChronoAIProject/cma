"""Missing owners burn versions; uncertain owners still refuse allocation."""
import copy
import json
import subprocess
import unittest
from unittest.mock import Mock

from staging_cd_abandoned_fixtures import BURNED, QUEUE
from staging_cd_allocate import allocate_queue, run_active
from staging_cd_common import Refusal
from staging_cd_github import GitHub, GitHubNotFound
from staging_cd_history import DevelopHistory
from staging_cd_reservations import next_reservation, pending_name
from staging_cd_versions import DEPLOYABLES, version_tuple


def response(code, body=None):
    if body is None:
        body = {"status": str(code)}
    return subprocess.CompletedProcess([], 0 if code == 200 else 1,
                                       f"HTTP/2.0 {code} fixture\r\nContent-Type: application/json\r\n\r\n"
                                       + json.dumps(body), "private diagnostic")


class AbandonedTests(unittest.TestCase):
    def reservation(self):
        return next_reservation("web", "a" * 40, "b" * 40, 1, "patch", 9, 1, [], [])

    def test_both_explicit_404s_are_abandoned(self):
        runner = Mock(return_value=response(404))
        self.assertFalse(run_active(GitHub("fixture", runner), self.reservation()))
        self.assertEqual(runner.call_count, 2)
        paths = [next(arg for arg in call.args[0] if arg.startswith("repos/"))
                 for call in runner.call_args_list]
        self.assertEqual(paths, ["repos/ChronoAIProject/cma/actions/runs/9/attempts/1",
                                 "repos/ChronoAIProject/cma/actions/runs/9"])
        for call in runner.call_args_list:
            self.assertIn("--include", call.args[0])
            self.assertIn("GET", call.args[0])
            self.assertIsNone(call.kwargs["input"])

    def test_auth_server_rate_limit_and_mixed_missing_results_refuse(self):
        for code in (200, 401, 403, 429, 500, 503):
            runner = Mock(side_effect=[response(404), response(code)])
            with self.subTest(parent_code=code), self.assertRaises(Refusal):
                run_active(GitHub("fixture", runner), self.reservation())
        for code in (401, 403, 429, 500, 503):
            runner = Mock(return_value=response(code))
            with self.subTest(attempt_code=code), self.assertRaises(Refusal):
                run_active(GitHub("fixture", runner), self.reservation())
            self.assertEqual(runner.call_count, 1)

    def test_404_cannot_be_inferred_from_body_stderr_or_invalid_headers(self):
        failures = [
            subprocess.CompletedProcess([], 1, '{"status":"404"}', "HTTP 404"),
            subprocess.CompletedProcess([], 1, 'HTTP/2.0 404 missing\n\nnot JSON', ""),
            response(404, {"status": "403"}),
            response(403, {"status": "404"}),
            subprocess.CompletedProcess([], 0, 'HTTP/2.0 200 OK\n\n{"status":"404"}', ""),
            subprocess.CompletedProcess([], 2, 'HTTP/2.0 404 missing\n\n{"status":"404"}', ""),
        ]
        for result in failures:
            with self.subTest(result=result), self.assertRaises(Refusal):
                run_active(GitHub("fixture", Mock(return_value=result)), self.reservation())
        for error in (OSError("private"), subprocess.TimeoutExpired("fixture", 1)):
            with self.subTest(error=type(error)), self.assertRaises(Refusal):
                run_active(GitHub("fixture", Mock(side_effect=error)), self.reservation())

    def test_404_is_not_optional_for_writes_or_default_reads(self):
        runner = Mock(return_value=response(404))
        api = GitHub("fixture", runner)
        with self.assertRaises(Refusal) as caught:
            api.request("actions/runs/9")
        self.assertNotIsInstance(caught.exception, GitHubNotFound)
        runner.reset_mock()
        with self.assertRaises(Refusal):
            api.request("git/tags", {"tag": "fixture"}, allow_not_found=True)
        runner.assert_not_called()

    def fixture(self, *, pr_evidence=True, owner_status=404):
        self.assertEqual(len(BURNED), 70)
        pending = [copy.deepcopy(r) for r in BURNED if r["component"] in DEPLOYABLES]
        queue = copy.deepcopy(QUEUE)
        for change in queue["changes"]:
            change["deployables"] = [name for name in change["deployables"] if name in DEPLOYABLES.values()]
        # Independent PR provenance is supplied explicitly by this synthetic
        # fixture; abandonment must never bypass the real merged-PR gate.
        prs = {c["source"]: {"number": i + 1, "merged": True, "merged_at": "fixture",
               "merge_commit_sha": c["source"], "base": {"ref": "develop", "repo": {"full_name": "ChronoAIProject/cma"}},
               "labels": [{"name": "version-upgrade:patch"}]} for i, c in enumerate(queue["changes"])}
        def runner(args, **_kwargs):
            path = next(arg for arg in args if arg.startswith("repos/"))[len("repos/ChronoAIProject/cma/"):]
            if path.startswith("actions/runs/"):
                return response(owner_status)
            if path.startswith("commits/"):
                data = [prs[path.split("/")[1]]] if pr_evidence else []
            elif path.startswith("pulls/"):
                data = next(pr for pr in prs.values() if pr["number"] == int(path.split("/")[1]))
            else:
                raise AssertionError("Unexpected API read")
            return subprocess.CompletedProcess(args, 0, json.dumps(data), "")
        api = GitHub("fixture", runner)
        api.tags = lambda: []
        api.history = lambda: DevelopHistory(queue["head"], [c["source"] for c in reversed(queue["changes"])])
        reservations = Mock()
        reservations.list.side_effect = lambda: copy.deepcopy(pending)
        reservations.insert.side_effect = lambda value: pending.append(copy.deepcopy(value)) is None
        return queue, api, reservations, pending

    def test_large_burned_inventory_allocates_each_change_above_watermark(self):
        queue, api, reservations, pending = self.fixture()
        before = copy.deepcopy(pending)
        watermarks = {c: max((r["version"] for r in before if r["component"] == c), key=version_tuple)
                      for c in DEPLOYABLES}
        result = allocate_queue(queue, api, reservations, 999, 1)
        tasks = [t for group in result["include"] for t in json.loads(group["tasks"])]
        self.assertEqual(len(tasks), sum(len(c["deployables"]) for c in queue["changes"]))
        self.assertTrue(all(t["mode"] == "publish" for t in tasks))
        self.assertEqual(pending[:len(before)], before)
        used = {pending_name(r) for r in before}
        for task in tasks:
            r = task["reservation"]
            self.assertEqual(r["previous"], watermarks[r["component"]])
            self.assertEqual(version_tuple(r["version"]), (*version_tuple(r["previous"])[:2],
                                                          version_tuple(r["previous"])[2] + 1))
            self.assertNotIn(pending_name(r), used)
            self.assertEqual(r["run_id"], 999)
            watermarks[r["component"]] = r["version"]
            used.add(pending_name(r))
        self.assertEqual(set(reservations.method_calls[i][0] for i in range(len(reservations.method_calls))),
                         {"list", "insert"})

    def test_unknown_owner_and_missing_pr_evidence_refuse_before_insertion(self):
        for options, reason in (({"owner_status": 503}, "Reservation owner run could not be read"),
                                ({"owner_status": 403}, "Reservation owner run could not be read"),
                                ({"pr_evidence": False}, "Source must belong to exactly one merged develop PR")):
            queue, api, reservations, pending = self.fixture(**options)
            before = copy.deepcopy(pending)
            with self.subTest(options=options), self.assertRaisesRegex(Refusal, reason):
                allocate_queue(queue, api, reservations, 999, 1)
            reservations.insert.assert_not_called()
            self.assertEqual(pending, before)


if __name__ == "__main__":
    unittest.main()
