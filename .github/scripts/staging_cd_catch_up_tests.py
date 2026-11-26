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
from staging_cd_reservations import pending_name
from staging_cd_targets import TARGETS
from staging_cd_versions import DEPLOYABLES, REPOSITORY

COMPLETED = {"action": "completed", "workflow_run": {
    "path": catch_up.WORKFLOW, "event": "push", "head_branch": "develop",
    "head_repository": {"full_name": REPOSITORY}}}


def instant(_seconds):
    return None


def decide_raw(develop, controller=None, **when):
    """Decide long after the head settled, unless `when` says otherwise."""
    # The synthetic runs must carry the identities the check reads.
    assert (fixture.WORKFLOW, fixture.BOT["login"]) == (catch_up.WORKFLOW, catch_up.BOT)
    when = {"now": develop.later(60), **when}
    return catch_up.decide(develop.server, develop.refs, controller or develop.controller, **when)[:3]


def decide(develop, controller=None, **when):
    """The decision, with each lane that is not current as (component, reason)."""
    reason, run, lanes = decide_raw(develop, controller, **when)
    return reason, run, [(lane.component, lane.reason) for lane in lanes]


def rerun(develop, run):
    return catch_up.rerun(develop.server, run, lambda: develop.head, sleep=instant)


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
        # A build left behind is retried straight after the run that completes last.
        reason, run, lanes = decide(develop, now=develop.later(0))
        self.assertEqual((reason, run["id"], lanes), ("rerun", 802, [("runtime-omarchy", "unfinalized-reservation")]))
        self.assertEqual(rerun(develop, run), 2)
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
        rerun(develop, run)
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
        # The yielded claims stay current once a later push owns the head.
        third = develop.push({"web"})
        develop.finalize(third["web"])
        develop.deploy(803, "web")
        develop.complete(803)
        self.assertEqual(decide(develop), ("nothing-behind", None, []))

    def test_unallocated_or_already_caught_up_head_is_not_re_run(self):
        develop = Develop()
        develop.push({"runtime-omarchy"})
        develop.push({"web"})
        develop.complete(801, 802)
        develop.runs[802]["attempts"][-1]["allocate"] = "failure"
        self.assertEqual(decide(develop), ("head-unallocated", None, []))
        develop.runs[802]["attempts"][-1]["actor"] = BOT
        behind = [("runtime-omarchy", "unfinalized-reservation")]
        # A spent head is reported whatever its allocation did.
        reason, run, lanes = decide(develop)
        self.assertEqual((reason, run["run_attempt"], lanes), ("exhausted", 1, behind))
        develop.runs[802]["attempts"][-1]["allocate"] = "success"
        reason, run, lanes = decide(develop)
        self.assertEqual((reason, run["run_attempt"], lanes), ("exhausted", 1, behind))
        # A maintainer's later re-run may be caught up once more, within the cap.
        develop.runs[802]["attempts"][-1]["actor"] = HUMAN
        self.assertEqual(decide(develop)[0], "rerun")
        develop.runs[802]["attempts"].extend(dict(develop.runs[802]["attempts"][-1]) for _ in range(4))
        reason, run, lanes = decide(develop)
        self.assertEqual((reason, run["run_attempt"], lanes), ("exhausted", catch_up.ATTEMPTS, behind))
        self.assertEqual(develop.posts, [])

    def test_a_cancelled_head_still_re_runs_for_a_build_left_behind(self):
        develop = Develop()
        develop.push({"runtime-omarchy"})
        develop.finalize(develop.push({"web"})["web"])
        develop.deploy(802, "web", "failed")
        develop.complete(801, 802)
        # A job timeout cancels its run too, so the busy owner's lane is re-planned as before.
        develop.runs[802]["attempts"][-1]["conclusion"] = "cancelled"
        reason, run, lanes = decide(develop, now=develop.later(0))
        self.assertEqual((reason, run["id"], lanes), ("rerun", 802, [
            ("runtime-omarchy", "unfinalized-reservation"), ("web", "unproven-final")]))

    def test_a_cancelled_head_is_never_re_run_for_finals_alone(self):
        develop = Develop()
        develop.finalize(develop.push({"web"})["web"])
        develop.deploy(801, "web", "failed")
        develop.complete(801)
        develop.runs[801]["attempts"][-1]["conclusion"] = "cancelled"
        self.assertEqual(decide(develop), ("cancelled", None, [("web", "unproven-final")]))
        # A maintainer's re-run of it is a new attempt that may be caught up again.
        develop.start(801)
        develop.allocate(801)
        develop.complete(801)
        self.assertEqual(decide(develop)[::2], ("rerun", [("web", "unproven-final")]))
        self.assertEqual(develop.posts, [])

    def test_a_plan_stopped_at_its_gate_is_re_run_once_after_staging_settles(self):
        develop = Develop()
        develop.push({"web"})
        develop.complete(801)
        attempt = develop.runs[801]["attempts"][-1]
        attempt.update(allocate=None, plan="stopped")

        def check(minutes):
            return decide_raw(develop, now=develop.later(minutes))
        # Let the transient failure clear before re-running the plan.
        self.assertEqual(check(0), ("plan-waiting", None, []))
        self.assertEqual(check(9), ("plan-waiting", None, []))
        reason, run, lanes = check(10)
        self.assertEqual((reason, run["id"], lanes), ("plan-rerun", 801, []))
        # A plan that failed in its own step (a refusal or an error) waits for the next push.
        attempt["plan"] = "failed"
        self.assertEqual(check(10), ("head-unallocated", None, []))
        # A cancelled head is never re-run automatically.
        attempt.update(plan="stopped", conclusion="cancelled")
        self.assertEqual(check(10), ("cancelled", None, []))
        del attempt["conclusion"]
        # Once re-run by the check itself, a plan that stops again is exhausted: never a second time.
        attempt["actor"] = BOT
        reason, run, lanes = check(10)
        self.assertEqual((reason, run["id"], lanes), ("plan-exhausted", 801, []))
        attempt["actor"] = HUMAN
        attempt["updated"] = "not a time"
        self.assertEqual(check(10), ("plan-waiting", None, []))
        self.assertEqual(develop.posts, [])

    def test_a_bot_triggered_head_with_a_stopped_plan_is_never_re_run(self):
        develop = Develop()
        develop.push({"runtime-omarchy"})  # Run 801 is still building when the next push plans.
        develop.push({"web"})
        develop.complete(801, 802)
        attempt = develop.runs[802]["attempts"][-1]
        attempt.update(allocate=None, plan="stopped")

        def check():
            return decide_raw(develop, now=develop.later(10))
        self.assertEqual(check()[0], "plan-rerun")
        # The check's own attempt spent this head's automatic re-run: its stopped plan is exhausted,
        # and a lane still behind is reported with it.
        attempt["actor"] = BOT
        reason, run, lanes = check()
        self.assertEqual((reason, run["id"], [lane.component for lane in lanes]),
                         ("plan-exhausted", 802, ["runtime-omarchy"]))
        self.assertNotIn(reason, catch_up.RERUNS)
        self.assertEqual(develop.posts, [])

    def test_an_allocation_stopped_at_its_gate_is_re_run_once_after_staging_settles(self):
        develop = Develop()
        develop.push({"web"})
        develop.complete(801)
        attempt = develop.runs[801]["attempts"][-1]
        attempt["allocate"] = "stopped"

        def check(minutes):
            return decide_raw(develop, now=develop.later(minutes))
        # Let the transient failure clear before re-running the allocation.
        self.assertEqual(check(0), ("allocation-waiting", None, []))
        self.assertEqual(check(9), ("allocation-waiting", None, []))
        reason, run, lanes = check(10)
        self.assertEqual((reason, run["id"], lanes), ("allocation-rerun", 801, []))
        self.assertIn(reason, catch_up.RERUNS)
        # A refusal that needs a person failed the allocation step itself: it waits for the next push.
        attempt["allocate"] = "failure"
        self.assertEqual(check(10), ("head-unallocated", None, []))
        # A cancelled head is never re-run automatically.
        attempt.update(allocate="stopped", conclusion="cancelled")
        self.assertEqual(check(10), ("cancelled", None, []))
        del attempt["conclusion"]
        # Once re-run by the check itself, an allocation that stops again is exhausted: never a second time.
        attempt["actor"] = BOT
        self.assertEqual(check(10)[::2], ("allocation-exhausted", []))
        # A maintainer's re-run earns one more, never at the attempt cap.
        attempt["actor"] = HUMAN
        self.assertEqual(check(10)[0], "allocation-rerun")
        develop.runs[801]["attempts"].extend(dict(attempt) for _ in range(catch_up.ATTEMPTS - 1))
        reason, run, lanes = check(10)
        self.assertEqual((reason, run["run_attempt"], lanes), ("allocation-exhausted", catch_up.ATTEMPTS, []))
        self.assertEqual(develop.posts, [])

    def test_only_a_job_that_failed_at_its_gate_alone_is_a_transient_stop(self):
        def job(conclusion, steps):
            return {"status": "completed", "conclusion": conclusion,
                    "steps": [{"name": step, "conclusion": outcome} for step, outcome in steps]}
        for stage, gate, work in (
                ("plan", catch_up.PLAN_GATE, "Reconcile first-parent merges with the shared selector"),
                ("allocation", catch_up.ALLOCATION_GATE, "Reserve independent immutable component versions")):
            for case, shape, reason in (
                    ("gate alone", job("failure", [(work, "success"), (gate, "failure")]), stage + "-rerun"),
                    # The gate and another step failed: a refusal or an error, never a transient stop.
                    ("gate and step", job("failure", [(work, "failure"), (gate, "failure")]), "head-unallocated"),
                    # A failed gate in a job that did not fail (a cancelled one) is no stop either.
                    ("job cancelled", job("cancelled", [(work, "success"), (gate, "failure")]),
                     "head-unallocated")):
                with self.subTest(stage=stage, case=case):
                    develop = Develop()
                    develop.push({"web"})
                    develop.complete(801)
                    attempt = develop.runs[801]["attempts"][-1]
                    attempt.update(allocate=None, plan=shape) if stage == "plan" else attempt.update(allocate=shape)
                    self.assertEqual(decide(develop)[0], reason)
                    self.assertEqual(develop.posts, [])

    def test_a_re_run_attempt_takes_over_reservations_its_stopped_allocation_committed(self):
        # Run 44948149418: the atomic reservation push outlived its client bound
        # and settlement found nothing, so allocation stopped at its gate. The
        # server committed the whole push moments later.
        develop = Develop()
        first = develop.push({"am", "web"})
        develop.runs[801]["attempts"][-1]["allocate"] = "stopped"
        develop.complete(801)
        burned = {key: task["reservation"] for key, task in first.items()}
        self.assertTrue(all(pending_name(value) in develop.server.refs for value in burned.values()))
        reason, run, lanes = decide_raw(develop)
        self.assertEqual((reason, run["id"], lanes), ("allocation-rerun", 801, []))
        self.assertEqual(rerun(develop, run), 2)
        self.assertEqual(decide(develop), ("run-active", None, []))
        # The stopped attempt has settled, so the re-run attempt reserves each
        # source again after the burned version, which stays on the remote.
        retried = develop.allocate(801)
        self.assertEqual({key: task["mode"] for key, task in retried.items()}, {"am": "publish", "web": "publish"})
        for key, task in retried.items():
            reservation = task["reservation"]
            self.assertEqual((reservation["source"], reservation["previous"], reservation["run_id"],
                              reservation["run_attempt"]), (develop.source(0), burned[key]["version"], 801, 2))
            self.assertIn(pending_name(burned[key]), develop.server.refs)
        for key, task in retried.items():
            develop.finalize(task)
            develop.deploy(801, key)
        develop.complete(801)
        self.assertEqual(decide(develop), ("head-caught-up", None, []))
        self.assertEqual(develop.posts, [801])

    def test_one_automatic_re_run_per_head_is_shared_by_every_trigger(self):
        def stop(attempt, stage):
            attempt.update(allocate=None, plan="stopped") if stage == "plan" else attempt.update(allocate="stopped")

        # A stopped plan or allocation spends the head's re-run; the bot's attempt
        # is never re-run for the same or the other stop, or for a final it
        # leaves undeployed. A second stop is reported as exhausted.
        for first, then in (("plan", "plan"), ("plan", "allocation"), ("allocation", "allocation"),
                            ("allocation", "plan"), ("allocation", "final")):
            with self.subTest(first=first, then=then):
                develop = Develop()
                develop.push({"web"})
                stop(develop.runs[801]["attempts"][-1], first)
                develop.complete(801)
                reason, run, _lanes = decide_raw(develop)
                self.assertEqual(reason, first + "-rerun")
                rerun(develop, run)
                if then == "final":
                    develop.finalize(develop.allocate(801)["web"])
                    develop.deploy(801, "web", "failed")
                else:
                    stop(develop.runs[801]["attempts"][-1], then)
                develop.complete(801)
                reason, run, lanes = decide(develop)
                self.assertNotIn(reason, catch_up.RERUNS)
                self.assertEqual((reason, lanes), ("exhausted", [("web", "unproven-final")]) if then == "final"
                                 else (then + "-exhausted", []))
                self.assertEqual(develop.posts, [801])
        # A re-run for a final spends it too: the bot's stopped allocation is not re-run.
        develop = Develop()
        develop.finalize(develop.push({"web"})["web"])
        develop.deploy(801, "web", "failed")
        develop.complete(801)
        reason, run, _lanes = decide_raw(develop)
        self.assertEqual(reason, "rerun")
        rerun(develop, run)
        stop(develop.runs[801]["attempts"][-1], "allocation")
        develop.complete(801)
        reason, run, lanes = decide(develop)
        self.assertEqual((reason, run["run_attempt"], lanes), ("allocation-exhausted", 2, [("web", "unproven-final")]))
        self.assertEqual(develop.posts, [801])

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

    def test_final_source_names_the_verified_source_of_a_final(self):
        from staging_cd_catch_up import final_source
        develop = Develop()
        receipt = develop.finalize(develop.push({"web"})["web"])
        version = receipt["identity"]["version"]
        oid = develop.server.refs[receipt["tag"]]
        self.assertEqual(final_source(develop.server, "web", version, oid), develop.source(0))
        with self.assertRaisesRegex(Refusal, "Release tag identity differs"):
            final_source(develop.server, "eg", version, oid)

    def test_re_run_is_written_once_and_verified_by_reading_back(self):
        develop = Develop()
        develop.push({"runtime-omarchy"})
        develop.push({"web"})
        develop.complete(801, 802)
        _reason, run, _lanes = decide(develop)
        develop.landing = False
        with self.assertRaisesRegex(Refusal, "Catch-up re-run was not verified"):
            rerun(develop, run)
        self.assertEqual(develop.posts, [802])
        develop.start(802)
        with self.assertRaisesRegex(Refusal, "changed before its catch-up"):
            rerun(develop, run)
        self.assertEqual(develop.posts, [802])

    def test_a_new_develop_push_between_decision_and_write_skips_the_old_attempt(self):
        develop = Develop()
        develop.push({"runtime-omarchy"})
        develop.push({"web"})
        develop.complete(801, 802)
        reason, run, _lanes = decide(develop)
        self.assertEqual((reason, run["id"]), ("rerun", 802))
        develop.push(set())
        self.assertNotEqual(develop.head, run["head_sha"])
        self.assertIsNone(rerun(develop, run))
        self.assertEqual(develop.posts, [])

    def test_write_and_readback_bind_the_same_workflow_and_source(self):
        develop = Develop()
        develop.push({"web"})
        develop.complete(801)
        run = develop.view(801)
        for changed in ({"path": ".github/workflows/ci.yml"}, {"head_sha": "a" * 40}):
            with self.subTest(changed=changed):
                api = Mock()
                api.request.return_value = dict(run, **changed)
                with self.assertRaisesRegex(Refusal, "changed before its catch-up"):
                    catch_up.rerun(api, run, lambda: develop.head, sleep=instant)
                api.request.assert_called_once_with("actions/runs/801")

                api.request.reset_mock()
                api.request.side_effect = [run, None, dict(run, run_attempt=2, **changed)]
                with self.assertRaisesRegex(Refusal, "Develop head run identity is invalid"):
                    catch_up.rerun(api, run, lambda: develop.head, sleep=instant)
                self.assertEqual([call.args[0] for call in api.request.call_args_list],
                                 ["actions/runs/801", "actions/runs/801/rerun", "actions/runs/801"])

    def test_retry_decisions_refuse_partial_or_cross_attempt_job_evidence(self):
        develop = Develop()
        develop.push({"web"})
        develop.runs[801]["attempts"][-1].update(allocate=None, plan="stopped")
        develop.complete(801)
        self.assertEqual(decide(develop)[0], "plan-rerun")
        jobs_path = "actions/runs/801/attempts/1/jobs?per_page=100"
        full = develop.serve(jobs_path)["jobs"]
        invalid = [
            ({"jobs": full}, "could not be read"),
            ({"jobs": full[:1], "total_count": 2}, "could not be read"),
            ({"jobs": [full[0], full[0]], "total_count": 2}, "identity differs"),
        ]
        for changed in ({"run_id": 802}, {"run_attempt": 2}, {"head_sha": "a" * 40}):
            invalid.append(({"jobs": [dict(full[0], **changed), full[1]], "total_count": 2}, "identity differs"))
        for listing, error in invalid:
            with self.subTest(listing=listing):
                def request(url, *args, **kwargs):
                    return listing if url == jobs_path else develop.serve(url, *args, **kwargs)

                with patch.object(develop.server, "request", side_effect=request), self.assertRaisesRegex(Refusal, error):
                    decide(develop)
        self.assertEqual(develop.posts, [])


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
        reason, run, lanes = decide_raw(develop)
        self.assertEqual((reason, run["id"], lanes),
                         ("rerun", 801, [Lane("web", "unproven-final", version(develop, "web"))]))
        self.assertEqual(rerun(develop, run), 2)
        self.assertEqual(decide(develop), ("run-active", None, []))
        # The re-run attempt plans from the same controller outcome and resumes the final.
        retried = develop.allocate(801)
        self.assertEqual(retried, {"web": {"component": "web", "mode": "resume", "receipt": develop.finals["web"]}})
        develop.deploy(801, "web")
        develop.complete(801)
        self.assertEqual(decide(develop), ("head-caught-up", None, []))
        self.assertEqual(develop.posts, [801])

    def test_a_deploy_failure_is_retried_only_after_its_head_settled(self):
        develop = self.failed_deploy()
        develop.complete(801)
        behind = [("web", "unproven-final")]
        # Completion and the earlier recheck leave the cause time to clear.
        for minutes in (0, 9):
            with self.subTest(minutes=minutes):
                self.assertEqual(decide(develop, now=develop.later(minutes)),
                                 ("final-waiting", None, behind))
        self.assertEqual(decide(develop, now=develop.later(10))[::2], ("rerun", behind))
        develop.runs[801]["attempts"][-1]["updated"] = None
        self.assertEqual(decide(develop)[0], "final-waiting")
        self.assertEqual(develop.posts, [])

    def test_settling_is_measured_from_the_newest_staging_run_not_the_head(self):
        develop = Develop()
        first = develop.push({"runtime-omarchy", "web"})
        develop.finalize(develop.push({"eg"})["eg"])
        develop.deploy(802, "eg")
        develop.complete(802)
        develop.minutes += 45  # The head settled long ago; the older run 801 still deploys.
        for key in ("web", "runtime-omarchy"):
            develop.finalize(first[key])
        develop.deploy(801, "web", "failed")
        develop.deploy(801, "runtime-omarchy")
        develop.complete(801)
        behind = [("web", "unproven-final")]
        for minutes in (1, 9):
            self.assertEqual(decide(develop, now=develop.later(minutes)),
                             ("final-waiting", None, behind))
        reason, run, lanes = decide(develop, now=develop.later(10))
        self.assertEqual((reason, run["id"], lanes), ("rerun", 802, behind))

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
        _reason, run, _lanes = decide_raw(develop)
        rerun(develop, run)
        develop.allocate(801)
        develop.deploy(801, "web", "failed")
        develop.complete(801)
        # Every later check, after each run and on schedule, reports the lane without re-running.
        for _ in range(3):
            reason, run, lanes = decide_raw(develop)
            self.assertEqual((reason, run["run_attempt"], lanes),
                             ("exhausted", 2, [Lane("web", "unproven-final", version(develop, "web"))]))
        self.assertEqual(develop.posts, [801])
        # Maintainer re-runs each earn one more automatic attempt, never past the cap.
        attempts = develop.runs[801]["attempts"]
        attempts.extend(dict(attempts[-1], actor=HUMAN, jobs=[]) for _ in range(catch_up.ATTEMPTS - 2))
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
        # The build left behind re-runs at once and carries the final with it.
        reason, run, lanes = decide(develop, now=develop.later(0))
        self.assertEqual((reason, run["id"], lanes), ("rerun", 802, [
            ("runtime-omarchy", "unfinalized-reservation"), ("web", "unproven-final")]))
        # An unknown outcome never hides the reservation it cannot judge.
        self.assertEqual(decide(develop, lambda: None), ("rerun", run, [
            ("runtime-omarchy", "unfinalized-reservation"), ("eg", "controller-unknown"),
            ("web", "controller-unknown")]))
        rerun(develop, run)
        retried = develop.allocate(802)
        self.assertEqual({key: task["mode"] for key, task in retried.items()},
                         {"runtime-omarchy": "publish", "web": "resume"})

    def test_every_deployable_is_a_staging_target_and_no_other_final_is_judged(self):
        # A deployable no controller can prove would look undeployed after every push.
        self.assertEqual(set(DEPLOYABLES), TARGETS)
        develop = self.failed_deploy()
        develop.complete(801)
        with patch.object(catch_up, "TARGETS", TARGETS - {"web"}):
            self.assertEqual(decide(develop), ("nothing-behind", None, []))
        self.assertEqual(decide(develop)[::2], ("rerun", [("web", "unproven-final")]))

    def test_an_undeployed_final_needs_an_owner_strictly_newer_than_the_oldest_listed_writer(self):
        since = "2026-11-21T13:20:00Z"
        self.assertTrue(catch_up.undeployed(None, None))  # The listing holds every deployment.
        for started, expected in ((None, False), ("2026-11-21T13:19:59Z", False), (since, False),
                                  ("2026-11-21T13:20:01Z", True), ("13:20:01", False)):
            with self.subTest(started=started):
                owner = None if started is None else {"run_started_at": started}
                self.assertIs(catch_up.undeployed(owner, since), expected)
        self.assertFalse(catch_up.undeployed({"run_started_at": "2026-11-21T13:20:01Z"}, "unknown"))

    def test_final_no_writer_covered_since_its_owner_started_is_undeployed(self):
        develop = Develop()
        develop.finalize(develop.push({"web"})["web"])
        develop.complete(801)  # Admission failed before any staging writer started.
        self.assertEqual(decide(develop)[::2], ("rerun", [("web", "undeployed-final")]))
        develop.deploy(801, "web")
        # The web proof is on an older page than the newer Gateway writers.
        develop.finalize(develop.push({"eg"})["eg"])
        for _ in range(catch_up_window()):
            develop.deploy(802, "eg")
        develop.complete(802)
        self.assertEqual(decide(develop), ("nothing-behind", None, []))
        # The newly published final does not match the older controller proof.
        develop.finalize(develop.push({"web"})["web"])
        develop.complete(803)
        self.assertEqual(decide(develop)[::2], ("rerun", [("web", "unproven-final")]))
        self.assertEqual(develop.posts, [])


def catch_up_window():
    from staging_cd_promotion import PAGE_SIZE
    return PAGE_SIZE


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
        catch_up.authorized(self.environment("schedule", {"schedule": "3-59/10 * * * *"}))
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

    def test_write_fence_requests_only_the_public_develop_ref(self):
        head = "a" * 40
        runner = Mock(return_value=subprocess.CompletedProcess([], 0, f"{head}\trefs/heads/develop\n", ""))
        self.assertEqual(catch_up.advertised(runner, include_tags=False), {catch_up.DEVELOP: head})
        self.assertEqual(runner.call_args.args[0][5:], ["ls-remote", "--refs", catch_up.REMOTE, catch_up.DEVELOP])

    def run_main(self, decide=None, rerun=None, env=None, *, every=False):
        output = io.StringIO()
        decision = (lambda *args, **kwargs: catch_up.Decision(*decide(*args, **kwargs))) if decide else None
        with patch.dict("os.environ", env or self.environment(), clear=True), \
                patch.object(catch_up, "decide", side_effect=decision), \
                patch.object(catch_up, "rerun", side_effect=rerun), contextlib.redirect_stdout(output):
            status = catch_up.main()
        lines = output.getvalue().splitlines()
        return status, lines if every else lines[-1]

    def test_outcome_line_names_only_the_decision_run_and_lanes(self):
        self.assertEqual(self.run_main(lambda *_, **__: ("nothing-behind", None, [])), (0, "CATCH-UP nothing-behind"))
        run = {"id": 802, "run_attempt": 1}
        lanes = [Lane("am", "unfinalized-reservation", "0.20.12"), Lane("runtime-omarchy", "unproven-final", "0.18.12"),
                 Lane("web", "controller-unknown", "0.10.4")]
        self.assertEqual(self.run_main(lambda *_, **__: ("rerun", run, lanes), lambda *_: 2, every=True), (0, [
            "PASS catch-up", "CATCH-UP lane=am reason=unfinalized-reservation version=0.20.12",
            "CATCH-UP lane=runtime-omarchy reason=unproven-final version=0.18.12",
            "CATCH-UP lane=web reason=controller-unknown version=0.10.4",
            "CATCH-UP rerun run=802 attempt=2 lanes=am,runtime-omarchy"]))
        self.assertEqual(self.run_main(lambda *_, **__: ("controller-unknown", None, lanes[2:]), every=True), (0, [
            "PASS catch-up", "CATCH-UP lane=web reason=controller-unknown version=0.10.4", "CATCH-UP controller-unknown"]))
        self.assertEqual(self.run_main(lambda *_, **__: ("final-waiting", None, lanes[1:2]), every=True), (0, [
            "PASS catch-up", "CATCH-UP lane=runtime-omarchy reason=unproven-final version=0.18.12",
            "CATCH-UP final-waiting"]))

    def test_a_cancelled_head_is_reported_as_a_stop(self):
        self.assertEqual(self.run_main(lambda *_, **__: ("cancelled", None, [])), (0, "CATCH-UP stop reason=cancelled"))

    def test_a_moved_head_does_not_report_a_retry_or_stale_lanes(self):
        run = {"id": 802, "run_attempt": 1}
        lanes = [Lane("am", "unfinalized-reservation", "0.20.12")]
        self.assertEqual(self.run_main(lambda *_, **__: ("rerun", run, lanes), lambda *_: None, every=True),
                         (0, ["PASS catch-up", "CATCH-UP head-moved"]))

    def test_both_authorized_triggers_use_the_same_fresh_decision(self):
        for event in ("workflow_run", "schedule"):
            env = self.environment(event, COMPLETED if event == "workflow_run" else {"schedule": "3-59/10 * * * *"})
            decide = Mock(return_value=("nothing-behind", None, []))
            self.run_main(decide, env=env)
            self.assertEqual(decide.call_args.kwargs, {})

    def test_completion_event_rechecks_and_reruns_a_stopped_plan_without_a_schedule(self):
        develop = Develop()
        develop.push({"web"})
        develop.runs[801]["attempts"][-1].update(allocate=None, plan="stopped")
        develop.complete(801)
        env = self.environment()
        output_path = Path(env["GITHUB_EVENT_PATH"]).with_name("output")
        env["GITHUB_OUTPUT"] = str(output_path)

        def run_at(minutes):
            output = io.StringIO()
            with patch.dict("os.environ", env, clear=True), \
                    patch.object(catch_up, "GitHub", return_value=develop.server), \
                    patch.object(catch_up, "advertised", side_effect=lambda **_: develop.refs()), \
                    patch.object(catch_up, "controller_reader", return_value=develop.controller), \
                    patch.object(catch_up.time, "time", return_value=develop.later(minutes)), \
                    contextlib.redirect_stdout(output):
                self.assertEqual(catch_up.main(), 0)
            return output.getvalue().splitlines()[-1]

        deadline = f"settle_until={int(develop.later(10))}\n"
        self.assertEqual(run_at(9), "CATCH-UP plan-waiting")
        self.assertEqual(output_path.read_text(), deadline)
        self.assertEqual(develop.posts, [])
        self.assertEqual(run_at(10), "CATCH-UP plan-rerun run=801 attempt=2")
        self.assertEqual(develop.posts, [801])
        self.assertEqual(output_path.read_text(), deadline)

    def test_a_second_stop_at_the_same_gate_warns_and_is_never_re_run(self):
        for stage in ("allocation", "plan"):
            with self.subTest(stage=stage):
                develop = Develop()
                develop.push({"web", "am"})
                attempt = develop.runs[801]["attempts"][-1]
                attempt.update(allocate=None, plan="stopped") if stage == "plan" else attempt.update(allocate="stopped")
                develop.complete(801)
                _reason, run, _lanes = decide_raw(develop)
                rerun(develop, run)
                # The bot's attempt reserves the same sources again, then stops at the same gate.
                if stage == "allocation":
                    develop.allocate(801)
                    develop.runs[801]["attempts"][-1]["allocate"] = "stopped"
                else:
                    develop.runs[801]["attempts"][-1].update(allocate=None, plan="stopped")
                develop.complete(801)
                decision = decide_raw(develop)
                self.assertEqual((decision[0], decision[1]["run_attempt"]), (stage + "-exhausted", 2))
                never = Mock(side_effect=AssertionError("re-run"))
                status, lines = self.run_main(lambda *_, **__: decision, never, every=True)
                never.assert_not_called()
                self.assertEqual((status, lines), (0, [
                    "PASS catch-up",
                    f"::warning title=Staging CD catch-up exhausted::the {stage} of run 801 stopped at its "
                    "transient gate again after attempt 2; a push or a maintainer re-run retries it",
                    f"CATCH-UP {stage}-exhausted run=801 attempts=2"]))
                self.assertEqual(develop.posts, [801])

    def test_an_exhausted_head_names_each_lane_still_behind_and_its_attempts(self):
        run = {"id": 802, "run_attempt": 2}
        lanes = [Lane("runtime-omarchy", "unproven-final", "0.18.12"), Lane("web", "undeployed-final", "0.10.4")]
        status, lines = self.run_main(lambda *_, **__: ("exhausted", run, lanes), every=True)
        self.assertEqual((status, lines[:3], lines[4:]), (0, ["PASS catch-up",
            "CATCH-UP lane=runtime-omarchy reason=unproven-final version=0.18.12",
            "CATCH-UP lane=web reason=undeployed-final version=0.10.4"], [
            "CATCH-UP exhausted lane=runtime-omarchy attempts=2", "CATCH-UP exhausted lane=web attempts=2"]))
        self.assertEqual(lines[3], "::warning title=Staging CD catch-up exhausted::runtime-omarchy,web stay behind "
                                   "after attempt 2 of run 802; a push or a maintainer re-run retries them")
        status, lines = self.run_main(lambda *_, **__: ("exhausted", run, lanes[1:]), every=True)
        self.assertEqual(lines[2], "::warning title=Staging CD catch-up exhausted::web stays behind "
                                   "after attempt 2 of run 802; a push or a maintainer re-run retries it")

    def test_the_controller_is_read_from_the_configured_staging_environment(self):
        env = self.environment()
        with patch.object(catch_up, "read_snapshot", return_value="outcome") as read:
            self.assertEqual(catch_up.controller_reader(env)(), "outcome")
        read.assert_called_once_with("fixture", ENVIRONMENT, seconds=catch_up.CONTROLLER_SECONDS)
        # Allocation keeps its shorter budget and leaves time to recheck the fence.
        with patch("staging_cd_promotion.AllocationIO") as io, patch("staging_cd_promotion.GitHub"), \
                patch("staging_cd_promotion.snapshot") as snapshot:
            catch_up.read_snapshot("fixture", ENVIRONMENT)
            snapshot.call_args.kwargs["fence_request"]("deployments")
        self.assertEqual([call.kwargs["seconds"] for call in io.call_args_list], [8, 2])

    def test_a_stopped_plan_re_run_names_its_run_and_attempt_and_only_named_decisions_re_run(self):
        run = {"id": 802, "run_attempt": 1}
        self.assertEqual(self.run_main(lambda *_, **__: ("plan-rerun", run, []), lambda *_: 2),
                         (0, "CATCH-UP plan-rerun run=802 attempt=2"))
        self.assertEqual(self.run_main(lambda *_, **__: ("allocation-rerun", run, []), lambda *_: 2),
                         (0, "CATCH-UP allocation-rerun run=802 attempt=2"))
        self.assertEqual(catch_up.RERUNS, {"rerun", "plan-rerun", "allocation-rerun"})
        for waiting in ("plan-waiting", "allocation-waiting", "head-unallocated"):
            never = Mock(side_effect=AssertionError("re-run"))
            self.assertEqual(self.run_main(lambda *_, waiting=waiting, **__: (waiting, None, []), never),
                             (0, "CATCH-UP " + waiting))
            never.assert_not_called()
        never = Mock(side_effect=AssertionError("re-run"))
        status, _lines = self.run_main(lambda *_, **__: ("exhausted", run, [Lane("web", "undeployed-final", "0.10.4")]),
                                       never, every=True)
        self.assertEqual(status, 0)
        never.assert_not_called()

    def test_refusals_fail_with_their_fixed_message_and_unexpected_errors_stay_private(self):
        def unverified(*_args):
            raise Refusal("Catch-up re-run was not verified")
        run = {"id": 802, "run_attempt": 1}
        self.assertEqual(self.run_main(lambda *_, **__: ("rerun", run, [Lane("am", "unfinalized-reservation", "0.1.0")]),
                                       unverified),
                         (1, "FAIL catch-up Catch-up re-run was not verified"))
        def private(*_args, **_kwargs):
            raise KeyError("private response field")
        self.assertEqual(self.run_main(private), (1, "FAIL catch-up unexpected error"))
        self.assertEqual(self.run_main(env=self.environment("push")),
                         (1, "FAIL catch-up Catch-up requires its develop workflow"))


if __name__ == "__main__":
    unittest.main()
