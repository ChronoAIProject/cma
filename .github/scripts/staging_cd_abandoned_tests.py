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


def github(runner):
    # Transient reads repeat; a fake sleep keeps the bounded backoff instant.
    return GitHub("fixture", runner, retry={"sleep": lambda _seconds: None})


class AbandonedTests(unittest.TestCase):
    def reservation(self):
        return next_reservation("web", "a" * 40, "b" * 40, 1, "patch", 9, 1, [], [])

    def test_failed_web_writer_retries_current_head_while_am_still_owns_its_lane(self):
        web = self.reservation()
        am = next_reservation("am", web["source"], web["tree"], 1, "patch", 9, 1, [], [])
        head, tree = "c" * 40, "d" * 40
        queue = {"head": head, "event_source": head, "changes": [
            {"source": web["source"], "tree": web["tree"], "deployables": sorted([
                DEPLOYABLES["am"], DEPLOYABLES["web"],
            ])}
        ]}
        writes = Mock()
        writes.list.side_effect = lambda: [am, web]
        writes.final_after_owner.return_value = None
        api = Mock()
        api.tags.return_value = []
        api.history.return_value = DevelopHistory(head, [head, web["source"]])
        run = {"id": 9, "run_attempt": 1, "event": "push", "head_branch": "develop",
               "repository": {"full_name": "ChronoAIProject/cma"}, "status": "in_progress"}
        pr = {"number": 2, "merged": True, "merged_at": "fixture", "merge_commit_sha": head,
              "base": {"ref": "develop", "repo": {"full_name": "ChronoAIProject/cma"}},
              "labels": [{"name": "version-upgrade:patch"}]}
        web_job = {"name": "Publish protected Web / Protect Web rollout / Finalize protected Web / Publish",
                   "status": "completed", "conclusion": "failure", "run_id": 9, "run_attempt": 1}
        responses = {
            "actions/runs/9/attempts/1": run,
            "actions/runs/9/attempts/1/jobs?per_page=100": {
                "total_count": 2, "jobs": [web_job, {"name": "Release am / Publish", "status": "in_progress",
                                                   "run_id": 9, "run_attempt": 1}],
            },
            "git/commits/" + head: {"sha": head, "tree": {"sha": tree}},
            "commits/" + head + "/pulls?per_page=100&page=1": [pr],
            "pulls/2": pr,
        }
        api.request.side_effect = lambda path, **_kwargs: responses[path]
        result = allocate_queue(queue, api, writes, 10, 1)
        writes.final_after_owner.assert_called_once_with(web)
        writes.insert.assert_called_once()
        [group] = result["include"]
        [task] = json.loads(group["tasks"])
        attempted = task["reservation"]
        self.assertEqual((group["component"], attempted["source"], attempted["tree"], attempted["previous"]),
                         ("web", head, tree, web["version"]))

        # A successful publication can still finalize. An active one can still
        # publish. Both keep the reservation.
        for patch in ({"conclusion": "success"}, {"status": "in_progress", "conclusion": None}):
            with self.subTest(status=patch):
                web_job.update(patch)
                writes.reset_mock()
                self.assertEqual(allocate_queue(queue, api, writes, 10, 1), {"include": []})
                writes.insert.assert_not_called()
                writes.final_after_owner.assert_not_called()

    def test_both_explicit_404s_are_abandoned(self):
        runner = Mock(return_value=response(404))
        self.assertFalse(run_active(github(runner), self.reservation()))
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
            runner = Mock(side_effect=[response(404), *[response(code)] * 3])
            with self.subTest(parent_code=code), self.assertRaises(Refusal):
                run_active(github(runner), self.reservation())
        for code in (401, 403, 429, 500, 503):
            runner = Mock(return_value=response(code))
            with self.subTest(attempt_code=code), self.assertRaises(Refusal):
                run_active(github(runner), self.reservation())
            # Throttling and server errors repeat within the bound, then still refuse.
            self.assertEqual(runner.call_count, 3 if code in (429, 500, 503) else 1)

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
                run_active(github(Mock(return_value=result)), self.reservation())
        for error in (OSError("private"), subprocess.TimeoutExpired("fixture", 1)):
            with self.subTest(error=type(error)), self.assertRaises(Refusal):
                run_active(github(Mock(side_effect=error)), self.reservation())

    def test_reads_repeat_transport_failures_but_writes_run_once(self):
        reset = subprocess.CompletedProcess([], 1, "", "read: connection reset by peer")
        ok = subprocess.CompletedProcess([], 0, '{"id": 9}', "")
        runner = Mock(side_effect=[reset, subprocess.TimeoutExpired("gh", 60), ok])
        self.assertEqual(github(runner).request("actions/runs/9"), {"id": 9})
        self.assertEqual(runner.call_count, 3)
        for failure in (reset, subprocess.TimeoutExpired("gh", 60), response(503)):
            runner = Mock(side_effect=[failure, ok])
            with self.subTest(failure=failure), self.assertRaises(Refusal):
                github(runner).request("git/refs", {"ref": "refs/tags/fixture", "sha": "a" * 40})
            runner.assert_called_once()
        # A transport failure carries no HTTP response, so it is never a missing resource.
        runner = Mock(side_effect=[reset, response(404)])
        with self.assertRaises(GitHubNotFound):
            github(runner).request("actions/runs/9", allow_not_found=True)
        self.assertEqual(runner.call_count, 2)

    def test_reads_carry_headers_and_repeat_only_explicitly_throttled_403s(self):
        ok = subprocess.CompletedProcess([], 0, 'HTTP/2.0 200 OK\r\nX-RateLimit-Remaining: 0\r\n\r\n{"id": 9}', "")
        def forbidden(remaining):
            return subprocess.CompletedProcess(
                [], 1, f'HTTP/2.0 403 Forbidden\r\nX-RateLimit-Remaining: {remaining}\r\n\r\n{{"message": "x"}}',
                "gh: API rate limit exceeded (HTTP 403)")
        runner = Mock(side_effect=[forbidden(0), ok])
        self.assertEqual(github(runner).request("actions/runs/9"), {"id": 9})
        self.assertEqual(runner.call_count, 2)
        args = runner.call_args.args[0]
        self.assertEqual(args[-1], "repos/ChronoAIProject/cma/actions/runs/9")
        self.assertIn("--include", args)
        # The same message without the exhausted quota header is a permission refusal.
        runner = Mock(side_effect=[forbidden(4999), ok])
        with self.assertRaisesRegex(Refusal, "GitHub API request failed"):
            github(runner).request("actions/runs/9")
        runner.assert_called_once()

    def test_read_budget_shortens_attempts_and_refuses_once_spent(self):
        now = [0.0]
        timeouts = []
        def runner(args, **kwargs):
            timeouts.append(kwargs["timeout"])
            now[0] += kwargs["timeout"]
            raise subprocess.TimeoutExpired("gh", kwargs["timeout"])
        policy = {"clock": lambda: now[0], "sleep": lambda seconds: now.__setitem__(0, now[0] + seconds),
                  "rand": lambda: 1.0}
        # 60 s, a 1 s wait, then only the 39 s left; no wait may reach the budget.
        with self.assertRaisesRegex(Refusal, "GitHub API request failed"):
            GitHub("fixture", runner, retry=policy, deadline=100).request("actions/runs/9")
        self.assertEqual((timeouts, now[0]), ([60, 39], 100))
        # A per-read budget can only shorten the instance budget.
        timeouts.clear()
        now[0] = 0.0
        with self.assertRaises(Refusal):
            GitHub("fixture", runner, retry=policy, deadline=100).request("actions/runs/9", deadline=25)
        self.assertEqual(timeouts, [25])
        # A spent budget refuses without starting gh, and never at a write.
        now[0] = 100.0
        runner = Mock(return_value=subprocess.CompletedProcess([], 0, "{}", ""))
        with self.assertRaisesRegex(Refusal, "budget expired"):
            GitHub("fixture", runner, retry=policy, deadline=100).request("actions/runs/9")
        runner.assert_not_called()
        GitHub("fixture", runner, retry=policy, deadline=100).request("git/refs", {"ref": "fixture"})
        self.assertEqual(runner.call_args.kwargs["timeout"], 60)

    def test_404_is_not_optional_for_writes_or_default_reads(self):
        runner = Mock(return_value=response(404))
        api = github(runner)
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
        api = github(runner)
        api.tags = lambda: []
        api.history = lambda: DevelopHistory(queue["head"], [c["source"] for c in reversed(queue["changes"])])
        reservations = Mock()
        reservations.list.side_effect = lambda: copy.deepcopy(pending)
        reservations.insert.side_effect = lambda value: pending.append(copy.deepcopy(value)) is None
        return queue, api, reservations, pending

    def test_large_burned_inventory_allocates_only_newest_per_component_above_watermark(self):
        queue, api, reservations, pending = self.fixture()
        before = copy.deepcopy(pending)
        watermarks = {c: max((r["version"] for r in before if r["component"] == c), key=version_tuple)
                      for c in DEPLOYABLES}
        result = allocate_queue(queue, api, reservations, 999, 1)
        tasks = [t for group in result["include"] for t in json.loads(group["tasks"])]
        self.assertEqual(len(tasks), len({name for c in queue["changes"] for name in c["deployables"]}))
        newest = {name: c["source"] for c in queue["changes"] for name in c["deployables"]}
        self.assertTrue(all(t["reservation"]["source"] == newest[DEPLOYABLES[t["component"]]] for t in tasks))
        self.assertTrue(all(t["mode"] == "publish" for t in tasks))
        self.assertEqual(pending[:len(before)], before)
        used = {pending_name(r) for r in before}
        for r in pending[len(before):]:
            self.assertEqual(r["previous"], watermarks[r["component"]])
            self.assertEqual(version_tuple(r["version"]), (*version_tuple(r["previous"])[:2],
                                                          version_tuple(r["previous"])[2] + 1))
            self.assertNotIn(pending_name(r), used)
            self.assertEqual(r["run_id"], 999)
            watermarks[r["component"]] = r["version"]
            used.add(pending_name(r))
        for task in tasks:
            self.assertEqual(task["reservation"]["version"], watermarks[task["component"]])
        self.assertEqual(set(reservations.method_calls[i][0] for i in range(len(reservations.method_calls))),
                         {"list", "insert"})

    def test_unknown_owner_and_missing_pr_evidence_refuse_before_insertion(self):
        for options, reason in (({"owner_status": 503}, "Reservation owner run could not be read"),
                                ({"owner_status": 403}, "Reservation owner run could not be read"),
                                ({"pr_evidence": False}, "Source must belong to exactly one merged develop PR")):
            queue, api, reservations, pending = self.fixture(**options)
            if "owner_status" in options:
                change = queue["changes"][-1]
                pending.append(next_reservation("eg", change["source"], change["tree"], 11,
                                                "patch", 123, 1, [], pending))
            before = copy.deepcopy(pending)
            with self.subTest(options=options), self.assertRaisesRegex(Refusal, reason):
                allocate_queue(queue, api, reservations, 999, 1)
            reservations.insert.assert_not_called()
            self.assertEqual(pending, before)


if __name__ == "__main__":
    unittest.main()
