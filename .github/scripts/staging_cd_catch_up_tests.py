"""A lane a failed run left behind is re-planned once; nothing else re-runs staging."""
import contextlib
import io
import json
from pathlib import Path
import subprocess
import tempfile
import unittest
from unittest.mock import Mock, patch

import staging_cd_behind_fixture as fixture
import staging_cd_catch_up as catch_up
from staging_cd_behind_fixture import BOT, ENVIRONMENT, HUMAN, Develop
from staging_cd_catch_up import Lane
from staging_cd_common import Refusal
from staging_cd_compatibility import GROUP
from staging_cd_history import DevelopHistory
from staging_cd_versions import REPOSITORY

COMPLETED = {"action": "completed", "workflow_run": {
    "path": catch_up.WORKFLOW, "event": "push", "head_branch": "develop",
    "head_repository": {"full_name": REPOSITORY}}}


def instant(_seconds):
    return None


def decide(develop, controller=None):
    """The decision, with each lane that is not current as (component, reason)."""
    # The synthetic runs must carry the identities the check reads.
    assert (fixture.WORKFLOW, fixture.BOT["login"]) == (catch_up.WORKFLOW, catch_up.BOT)
    reason, run, lanes = catch_up.decide(develop.server, develop.refs, controller or develop.controller)
    return reason, run, [(lane.component, lane.reason) for lane in lanes]


def version(develop, component):
    return develop.finals[component]["identity"]["version"]


class CatchUpTests(unittest.TestCase):
    def test_busy_skip_then_owner_failure_re_plans_the_lane_once(self):
        develop = Develop()
        first = develop.push({"runtime-omarchy", "web"})
        develop.finalize(first["web"])
        develop.deploy(801, "web")
        # Run 801 is still building runtime-omarchy, so the next push skips it.
        second = develop.push({"web"})
        self.assertEqual(second.keys(), {"web"})
        develop.finalize(second["web"])
        develop.deploy(802, "web")
        develop.complete(802)
        # The last run to finish decides; 801 is still running.
        self.assertEqual(decide(develop), ("run-active", None, []))
        develop.complete(801)  # Its runtime-omarchy build failed; no push follows.
        reason, run, lanes = decide(develop)
        self.assertEqual((reason, run["id"], lanes), ("rerun", 802, [("runtime-omarchy", "unfinalized-reservation")]))
        self.assertEqual(catch_up.rerun(develop.server, run, sleep=instant), 2)
        self.assertEqual(develop.posts, [802])
        # A pending or running re-run is the one pending catch-up.
        self.assertEqual(decide(develop), ("run-active", None, []))
        # The re-run attempt is the same develop push; it re-plans the lane at its source.
        retried = develop.allocate(802, verified={"web"})
        self.assertEqual(retried.keys(), {"runtime-omarchy"})
        reservation = retried["runtime-omarchy"]["reservation"]
        self.assertEqual((reservation["source"], reservation["previous"], reservation["run_attempt"]),
                         (develop.source(0), first["runtime-omarchy"]["reservation"]["version"], 2))
        # If the retry fails too, it waits for the next push: one catch-up per head.
        develop.complete(802)
        self.assertEqual(decide(develop), ("head-caught-up", None, []))
        self.assertEqual(develop.posts, [802])

    def test_group_owner_failure_re_plans_the_whole_transition(self):
        develop = Develop()
        anchor = develop.source(0)
        first = develop.push(GROUP, transition=anchor)
        finals = {key: develop.finalize(first[key]) for key in GROUP - {"am"}}
        # The transition owner still builds AM: the next push leaves the whole group to it.
        second = develop.push({"web"}, transition=anchor)
        self.assertEqual(second.keys(), {"web"})
        develop.finalize(second["web"])
        develop.deploy(802, "web")
        develop.complete(802, 801)  # The AM build failed.
        reason, run, lanes = decide(develop)
        # The runtimes' finals wait for the whole group, so nothing has deployed them.
        self.assertEqual((reason, run["id"], lanes), ("rerun", 802, [
            ("am", "unfinalized-reservation"), ("runtime", "undeployed-final"),
            ("runtime-omarchy", "undeployed-final")]))
        catch_up.rerun(develop.server, run, sleep=instant)
        retried = develop.allocate(802, verified={"web"})
        self.assertEqual(retried.keys(), GROUP)
        self.assertEqual((retried["am"]["mode"], retried["am"]["reservation"]["source"]), ("publish", anchor))
        for key in GROUP - {"am"}:
            self.assertEqual(retried[key], {"component": key, "mode": "resume", "receipt": finals[key]})

    def test_nothing_behind_never_re_runs(self):
        develop = Develop()
        for task in develop.push({"web", "eg"}).values():
            develop.finalize(task)
        develop.deploy(801, "web")
        develop.deploy(801, "eg")
        develop.complete(801)
        self.assertEqual(decide(develop), ("nothing-behind", None, []))
        # A failure in the head's own run alone never triggers a re-run.
        develop.push({"web"})
        develop.complete(802)
        self.assertEqual(decide(develop), ("nothing-behind", None, []))
        self.assertEqual(develop.posts, [])

    def test_a_group_claim_that_yielded_to_its_finalized_peer_is_current(self):
        develop = Develop()
        anchor = develop.source(0)
        original = develop.push(GROUP, transition=anchor)
        successor = develop.push({"am"}, transition=anchor)
        for key in GROUP - {"am"}:
            # The newer group claims the same source; the started build finalizes first.
            self.assertEqual(successor[key]["reservation"]["source"], anchor)
            develop.finalize(original[key])
        develop.finalize(successor["am"])
        develop.deploy(802, "contract")
        develop.complete(801, 802)
        self.assertEqual(decide(develop), ("nothing-behind", None, []))

    def test_unallocated_or_already_caught_up_head_is_not_re_run(self):
        develop = Develop()
        develop.push({"runtime-omarchy"})
        develop.push({"web"})
        develop.complete(801, 802)
        develop.runs[802]["attempts"][-1]["allocate"] = "failure"
        self.assertEqual(decide(develop), ("head-unallocated", None, []))
        develop.runs[802]["attempts"][-1]["allocate"] = "success"
        develop.runs[802]["attempts"][-1]["actor"] = BOT
        behind = [("runtime-omarchy", "unfinalized-reservation")]
        reason, run, lanes = decide(develop)
        self.assertEqual((reason, run["run_attempt"], lanes), ("exhausted", 1, behind))
        # A maintainer's later re-run may be caught up once more, within the cap.
        develop.runs[802]["attempts"][-1]["actor"] = HUMAN
        self.assertEqual(decide(develop)[0], "rerun")
        develop.runs[802]["attempts"].extend(dict(develop.runs[802]["attempts"][-1]) for _ in range(4))
        reason, run, lanes = decide(develop)
        self.assertEqual((reason, run["run_attempt"], lanes), ("exhausted", catch_up.ATTEMPTS, behind))
        self.assertEqual(develop.posts, [])

    def test_owner_without_a_run_is_settled_and_an_unknown_head_waits(self):
        develop = Develop()
        develop.push({"runtime-omarchy"})
        develop.push({"web"})
        develop.complete(801, 802)
        del develop.runs[801]  # A deleted owner can never finish its build.
        self.assertEqual(decide(develop)[:1], ("rerun",))
        develop.runs[801] = {"head": develop.source(0), "attempts": [{"status": "in_progress", "actor": HUMAN,
                                                                      "allocate": "success", "started": develop.now(),
                                                                      "jobs": []}]}
        listing = develop.serve
        def hidden(path, payload=None, **kwargs):
            value = listing(path, payload, **kwargs)
            if path.startswith("actions/workflows/"):
                value = {"workflow_runs": [run for run in value["workflow_runs"] if run["id"] != 801]}
            return value
        develop.server.request = hidden
        # An owner still running beyond the listing decides at its completion.
        self.assertEqual(decide(develop), ("owner-active", None, []))
        develop.server.request = listing
        develop.complete(801)
        # A push whose run GitHub has not listed yet: that run's completion decides.
        develop.server._history = DevelopHistory(develop.source(2), [develop.source(index) for index in (2, 1, 0)])
        self.assertEqual(decide(develop), ("head-run-missing", None, []))

    def test_re_run_is_written_once_and_verified_by_reading_back(self):
        develop = Develop()
        develop.push({"runtime-omarchy"})
        develop.push({"web"})
        develop.complete(801, 802)
        _reason, run, _lanes = decide(develop)
        develop.landing = False
        with self.assertRaisesRegex(Refusal, "Catch-up re-run was not verified"):
            catch_up.rerun(develop.server, run, sleep=instant)
        self.assertEqual(develop.posts, [802])
        develop.start(802)
        with self.assertRaisesRegex(Refusal, "changed before its catch-up"):
            catch_up.rerun(develop.server, run, sleep=instant)
        self.assertEqual(develop.posts, [802])


class UnprovenFinalTests(unittest.TestCase):
    """A published final whose staging deployment failed is redeployed by one re-run."""

    def failed_deploy(self):
        develop = Develop()
        develop.finalize(develop.push({"web"})["web"])
        develop.deploy(801, "web", "failed")  # The controller left the app stuck.
        return develop

    def test_unproven_final_of_a_settled_owner_re_runs_the_head_once_and_is_resumed(self):
        develop = self.failed_deploy()
        develop.complete(801)
        reason, run, lanes = catch_up.decide(develop.server, develop.refs, develop.controller)
        self.assertEqual((reason, run["id"], lanes),
                         ("rerun", 801, [Lane("web", "unproven-final", version(develop, "web"))]))
        self.assertEqual(catch_up.rerun(develop.server, run, sleep=instant), 2)
        self.assertEqual(decide(develop), ("run-active", None, []))
        # The re-run attempt plans from the same controller outcome and resumes the final.
        retried = develop.allocate(801)
        self.assertEqual(retried, {"web": {"component": "web", "mode": "resume", "receipt": develop.finals["web"]}})
        develop.deploy(801, "web")
        develop.complete(801)
        self.assertEqual(decide(develop), ("head-caught-up", None, []))
        self.assertEqual(develop.posts, [801])

    def test_proven_or_superseded_final_never_re_runs(self):
        develop = self.failed_deploy()
        # A newer final replaced the failed one and its own run deployed it.
        develop.finalize(develop.push({"web"})["web"])
        develop.deploy(802, "web")
        develop.complete(801, 802)
        self.assertEqual(decide(develop), ("nothing-behind", None, []))
        # The head's own proof stands even after a later writer left none.
        for outcome in ("overtaken", "failed"):
            develop.deploy(801, "web", outcome) if outcome == "overtaken" else develop.redeploy(outcome)
            self.assertEqual(decide(develop), ("nothing-behind", None, []))
        # The head's own failed build leaves a newer reservation; it waits for the next push.
        develop.push({"web"})
        develop.complete(803)
        self.assertEqual(decide(develop), ("nothing-behind", None, []))
        self.assertEqual(develop.posts, [])

    def test_a_maintainers_redeployment_proves_every_current_final(self):
        develop = self.failed_deploy()
        develop.complete(801)
        develop.redeploy()
        self.assertEqual(decide(develop), ("nothing-behind", None, []))
        develop.redeploy("failed")
        self.assertEqual(decide(develop)[::2], ("rerun", [("web", "unproven-final")]))

    def test_unproven_final_waits_while_a_run_or_its_lanes_writer_is_active(self):
        develop = self.failed_deploy()
        self.assertEqual(decide(develop), ("run-active", None, []))
        develop.complete(801)
        develop.redeploy("running")
        self.assertEqual(decide(develop), ("deploy-active", None, []))
        develop.jobs[max(develop.jobs)].update(status="completed", conclusion="failure")
        self.assertEqual(decide(develop)[0], "rerun")
        # An owner still running beyond the listing decides at its completion.
        develop.push(set())
        develop.complete(802)
        develop.runs[801]["attempts"][-1]["status"] = "in_progress"
        listing = develop.serve
        def hidden(path, payload=None, **kwargs):
            value = listing(path, payload, **kwargs)
            if path.startswith("actions/workflows/"):
                value = {"workflow_runs": [run for run in value["workflow_runs"] if run["id"] != 801]}
            return value
        develop.server.request = hidden
        self.assertEqual(decide(develop), ("owner-active", None, []))
        develop.complete(801)
        self.assertEqual(decide(develop)[::2], ("rerun", [("web", "unproven-final")]))
        self.assertEqual(develop.posts, [])

    def test_a_deterministic_deploy_failure_is_retried_once_then_reported_exhausted(self):
        develop = self.failed_deploy()
        develop.complete(801)
        _reason, run, _lanes = catch_up.decide(develop.server, develop.refs, develop.controller)
        catch_up.rerun(develop.server, run, sleep=instant)
        develop.allocate(801)
        develop.deploy(801, "web", "failed")
        develop.complete(801)
        # Every later check, after each run and hourly, reports the lane without re-running.
        for _ in range(3):
            reason, run, lanes = catch_up.decide(develop.server, develop.refs, develop.controller)
            self.assertEqual((reason, run["run_attempt"], lanes),
                             ("exhausted", 2, [Lane("web", "unproven-final", version(develop, "web"))]))
        self.assertEqual(develop.posts, [801])
        # Maintainer re-runs each earn one more automatic attempt, never past the cap.
        attempts = develop.runs[801]["attempts"]
        attempts.extend(dict(attempts[-1], actor=HUMAN) for _ in range(catch_up.ATTEMPTS - 2))
        reason, run, _lanes = decide(develop)
        self.assertEqual((reason, run["run_attempt"]), ("exhausted", catch_up.ATTEMPTS))
        self.assertEqual(develop.posts, [801])

    def test_an_unknown_controller_outcome_never_re_runs_and_says_so(self):
        develop = self.failed_deploy()
        develop.complete(801)
        unknown = [("web", "controller-unknown")]

        def unreadable():
            raise Refusal("GitHub API request failed")
        for reader in (unreadable, Mock(side_effect=OSError("network")), lambda: None):
            with self.subTest(reader=reader):
                self.assertEqual(decide(develop, reader), ("controller-unknown", None, unknown))
        # A writer that starts while the outcome is read leaves it unknown.
        listing = develop.serve
        def racing(path, payload=None, **kwargs):
            rows = listing(path, payload, **kwargs)
            return [dict(rows[0], id=rows[0]["id"] + 1)] if path.endswith("per_page=1") else rows
        develop.server.request = racing
        self.assertEqual(decide(develop), ("controller-unknown", None, unknown))
        develop.server.request = listing
        # So does develop ancestry that cannot be verified.
        with patch.object(develop.server, "history_at", side_effect=Refusal("Develop ancestry is incomplete")):
            self.assertEqual(decide(develop), ("controller-unknown", None, unknown))
        self.assertEqual(develop.posts, [])
        self.assertEqual(decide(develop)[0], "rerun")

    def test_unproven_final_joins_a_lane_a_busy_owner_left_behind(self):
        develop = Develop()
        first = develop.push({"runtime-omarchy", "web"})
        develop.finalize(first["web"])
        develop.deploy(801, "web", "failed")
        # The next push skips runtime-omarchy and resumes the failed final, which fails again.
        second = develop.push({"eg"})
        self.assertEqual({key: task["mode"] for key, task in second.items()}, {"eg": "publish", "web": "resume"})
        develop.finalize(second["eg"])
        develop.deploy(802, "eg")
        develop.deploy(802, "web", "failed")
        develop.complete(802, 801)
        reason, run, lanes = decide(develop)
        self.assertEqual((reason, run["id"], lanes), ("rerun", 802, [
            ("runtime-omarchy", "unfinalized-reservation"), ("web", "unproven-final")]))
        # An unknown outcome never hides the reservation it cannot judge.
        self.assertEqual(decide(develop, lambda: None), ("rerun", run, [
            ("runtime-omarchy", "unfinalized-reservation"), ("eg", "controller-unknown"),
            ("web", "controller-unknown")]))
        catch_up.rerun(develop.server, run, sleep=instant)
        retried = develop.allocate(802)
        self.assertEqual({key: task["mode"] for key, task in retried.items()},
                         {"runtime-omarchy": "publish", "web": "resume"})

    def test_final_no_writer_covered_since_its_owner_started_is_undeployed(self):
        develop = Develop()
        develop.finalize(develop.push({"web"})["web"])
        develop.complete(801)  # Admission failed before any staging writer started.
        self.assertEqual(decide(develop)[::2], ("rerun", [("web", "undeployed-final")]))
        develop.deploy(801, "web")
        # Twelve newer writers push the web proof out of the listing; it is not behind.
        develop.finalize(develop.push({"eg"})["eg"])
        for _ in range(catch_up_window()):
            develop.deploy(802, "eg")
        develop.complete(802)
        self.assertEqual(decide(develop), ("nothing-behind", None, []))
        # A final published after the oldest listed writer has had no writer at all.
        develop.finalize(develop.push({"web"})["web"])
        develop.complete(803)
        self.assertEqual(decide(develop)[::2], ("rerun", [("web", "undeployed-final")]))
        self.assertEqual(develop.posts, [])


def catch_up_window():
    from staging_cd_promotion import LIMIT
    return LIMIT


class EntryTests(unittest.TestCase):
    def environment(self, event_name="workflow_run", event=COMPLETED):
        directory = tempfile.TemporaryDirectory()
        self.addCleanup(directory.cleanup)
        path = Path(directory.name) / "event.json"
        path.write_text(json.dumps(event))
        return {"GITHUB_REPOSITORY": REPOSITORY, "GITHUB_REF": catch_up.DEVELOP, "GITHUB_EVENT_NAME": event_name,
                "GITHUB_WORKFLOW_REF": f"{REPOSITORY}/{catch_up.CHECK}@{catch_up.DEVELOP}",
                "GITHUB_EVENT_PATH": str(path), "GH_TOKEN": "fixture", "CD_STAGING_ENVIRONMENT": ENVIRONMENT}

    def test_only_completed_develop_staging_runs_or_the_schedule_authorize_a_check(self):
        catch_up.authorized(self.environment())
        catch_up.authorized(self.environment("schedule", {"schedule": "23 * * * *"}))
        refused = [
            self.environment("push"),
            self.environment("workflow_dispatch"),
            dict(self.environment(), GITHUB_REF="refs/heads/main"),
            dict(self.environment(), GITHUB_REPOSITORY="fork/cma"),
            dict(self.environment(), GITHUB_WORKFLOW_REF=f"{REPOSITORY}/.github/workflows/staging-cd.yml@refs/heads/develop"),
            {key: value for key, value in self.environment().items() if key != "CD_STAGING_ENVIRONMENT"},
            dict(self.environment(), CD_STAGING_ENVIRONMENT="Staging Environment"),
            self.environment(event=dict(COMPLETED, action="requested")),
            self.environment(event=[]),
        ]
        for key, value in (("event", "pull_request"), ("head_branch", "main"),
                           ("path", ".github/workflows/ci.yml"), ("head_repository", {"full_name": "fork/cma"})):
            refused.append(self.environment(event=dict(COMPLETED, workflow_run=dict(COMPLETED["workflow_run"],
                                                                                   **{key: value}))))
        for env in refused:
            with self.subTest(env=env), self.assertRaises(Refusal):
                catch_up.authorized(env)

    def test_one_credential_free_advertisement_binds_develop_and_every_tag(self):
        head, tag = "a" * 40, "b" * 40
        runner = Mock(return_value=subprocess.CompletedProcess([], 0, f"{head}\trefs/heads/develop\n"
                                                                     f"{tag}\trefs/tags/cma-web/v0.10.1\n", ""))
        with patch.dict("os.environ", {"GH_TOKEN": "private", "GIT_ASKPASS": "private"}):
            refs = catch_up.advertised(runner)
        self.assertEqual(refs, {"refs/heads/develop": head, "refs/tags/cma-web/v0.10.1": tag})
        args, kwargs = runner.call_args.args[0], runner.call_args.kwargs
        self.assertEqual(args[:5], ["git", "-c", "credential.helper=", "-c", "http.extraHeader="])
        self.assertEqual(args[5:], ["ls-remote", "--refs", catch_up.REMOTE, "refs/heads/develop", "refs/tags/*"])
        self.assertFalse({"GH_TOKEN", "GIT_ASKPASS"} & kwargs["env"].keys())
        for stdout, code in ((f"{tag}\trefs/tags/cma-web/v0.10.1\n", 0),
                             (f"{head}\trefs/heads/develop\n{head}\trefs/heads/develop\n", 0),
                             (f"{head}\trefs/heads/main\n", 0), ("not-an-oid\trefs/heads/develop\n", 0),
                             (f"{head}\trefs/heads/develop\n", 128)):
            runner = Mock(return_value=subprocess.CompletedProcess([], code, stdout, ""))
            with self.subTest(stdout=stdout, code=code), self.assertRaises(Refusal):
                catch_up.advertised(runner)

    def run_main(self, decide=None, rerun=None, env=None, *, every=False):
        output = io.StringIO()
        with patch.dict("os.environ", env or self.environment(), clear=True), \
                patch.object(catch_up, "decide", side_effect=decide), \
                patch.object(catch_up, "rerun", side_effect=rerun), contextlib.redirect_stdout(output):
            status = catch_up.main()
        lines = output.getvalue().splitlines()
        return status, lines if every else lines[-1]

    def test_outcome_line_names_only_the_decision_run_and_lanes(self):
        self.assertEqual(self.run_main(lambda *_: ("nothing-behind", None, [])), (0, "CATCH-UP nothing-behind"))
        run = {"id": 802, "run_attempt": 1}
        lanes = [Lane("am", "unfinalized-reservation", "0.20.12"), Lane("runtime-omarchy", "unproven-final", "0.18.12"),
                 Lane("web", "controller-unknown", "0.10.4")]
        self.assertEqual(self.run_main(lambda *_: ("rerun", run, lanes), lambda *_: 2, every=True), (0, [
            "PASS catch-up", "CATCH-UP lane=am reason=unfinalized-reservation version=0.20.12",
            "CATCH-UP lane=runtime-omarchy reason=unproven-final version=0.18.12",
            "CATCH-UP lane=web reason=controller-unknown version=0.10.4",
            "CATCH-UP rerun run=802 attempt=2 lanes=am,runtime-omarchy"]))
        self.assertEqual(self.run_main(lambda *_: ("controller-unknown", None, lanes[2:]), every=True), (0, [
            "PASS catch-up", "CATCH-UP lane=web reason=controller-unknown version=0.10.4", "CATCH-UP controller-unknown"]))

    def test_an_exhausted_head_names_each_lane_still_behind_and_its_attempts(self):
        run = {"id": 802, "run_attempt": 2}
        lanes = [Lane("runtime-omarchy", "unproven-final", "0.18.12"), Lane("web", "undeployed-final", "0.10.4")]
        status, lines = self.run_main(lambda *_: ("exhausted", run, lanes), every=True)
        self.assertEqual((status, lines[:3], lines[4:]), (0, ["PASS catch-up",
            "CATCH-UP lane=runtime-omarchy reason=unproven-final version=0.18.12",
            "CATCH-UP lane=web reason=undeployed-final version=0.10.4"], [
            "CATCH-UP exhausted lane=runtime-omarchy attempts=2", "CATCH-UP exhausted lane=web attempts=2"]))
        self.assertTrue(lines[3].startswith("::warning title=Staging CD catch-up exhausted::runtime-omarchy,web "))

    def test_the_controller_is_read_from_the_configured_staging_environment(self):
        env = self.environment()
        with patch.object(catch_up, "read_snapshot", return_value="outcome") as read:
            self.assertEqual(catch_up.controller_reader(env)(), "outcome")
        read.assert_called_once_with("fixture", ENVIRONMENT)

    def test_refusals_fail_with_their_fixed_message_and_unexpected_errors_stay_private(self):
        def unverified(*_args):
            raise Refusal("Catch-up re-run was not verified")
        run = {"id": 802, "run_attempt": 1}
        self.assertEqual(self.run_main(lambda *_: ("rerun", run, [Lane("am", "unfinalized-reservation", "0.1.0")]),
                                       unverified),
                         (1, "FAIL catch-up Catch-up re-run was not verified"))
        def private(*_args):
            raise KeyError("private response field")
        self.assertEqual(self.run_main(private), (1, "FAIL catch-up unexpected error"))
        self.assertEqual(self.run_main(env=self.environment("push")),
                         (1, "FAIL catch-up Catch-up requires its develop workflow"))


if __name__ == "__main__":
    unittest.main()
