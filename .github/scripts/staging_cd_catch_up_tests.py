"""A lane a busy owner left behind is re-planned once; nothing else re-runs staging."""
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
from staging_cd_behind_fixture import BOT, HUMAN, Develop
from staging_cd_common import Refusal
from staging_cd_compatibility import GROUP
from staging_cd_history import DevelopHistory
from staging_cd_versions import REPOSITORY

COMPLETED = {"action": "completed", "workflow_run": {
    "path": catch_up.WORKFLOW, "event": "push", "head_branch": "develop",
    "head_repository": {"full_name": REPOSITORY}}}


def instant(_seconds):
    return None


def decide(develop):
    # The synthetic runs must carry the identities the check reads.
    assert (fixture.WORKFLOW, fixture.BOT["login"]) == (catch_up.WORKFLOW, catch_up.BOT)
    return catch_up.decide(develop.server, develop.refs)


class CatchUpTests(unittest.TestCase):
    def test_busy_skip_then_owner_failure_re_plans_the_lane_once(self):
        develop = Develop()
        first = develop.push({"runtime-omarchy", "web"})
        develop.finalize(first["web"])
        # Run 801 is still building runtime-omarchy, so the next push skips it.
        second = develop.push({"web"})
        self.assertEqual(second.keys(), {"web"})
        develop.finalize(second["web"])
        develop.complete(802)
        # The last run to finish decides; 801 is still running.
        self.assertEqual(decide(develop), ("run-active", None, []))
        develop.complete(801)  # Its runtime-omarchy build failed; no push follows.
        reason, run, lanes = decide(develop)
        self.assertEqual((reason, run["id"], lanes), ("rerun", 802, ["runtime-omarchy"]))
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
        develop.complete(802, 801)  # The AM build failed.
        reason, run, lanes = decide(develop)
        self.assertEqual((reason, run["id"], lanes), ("rerun", 802, ["am"]))
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
        self.assertEqual(decide(develop), ("head-caught-up", None, []))
        # A maintainer's later re-run may be caught up once more, within the cap.
        develop.runs[802]["attempts"][-1]["actor"] = HUMAN
        self.assertEqual(decide(develop)[0], "rerun")
        develop.runs[802]["attempts"].extend(dict(develop.runs[802]["attempts"][-1]) for _ in range(4))
        self.assertEqual(decide(develop), ("head-caught-up", None, []))
        self.assertEqual(develop.posts, [])

    def test_owner_without_a_run_is_settled_and_an_unknown_head_waits(self):
        develop = Develop()
        develop.push({"runtime-omarchy"})
        develop.push({"web"})
        develop.complete(801, 802)
        del develop.runs[801]  # A deleted owner can never finish its build.
        self.assertEqual(decide(develop)[:1], ("rerun",))
        develop.runs[801] = {"head": develop.source(0), "attempts": [{"status": "in_progress", "actor": HUMAN,
                                                                      "allocate": "success"}]}
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


class EntryTests(unittest.TestCase):
    def environment(self, event_name="workflow_run", event=COMPLETED):
        directory = tempfile.TemporaryDirectory()
        self.addCleanup(directory.cleanup)
        path = Path(directory.name) / "event.json"
        path.write_text(json.dumps(event))
        return {"GITHUB_REPOSITORY": REPOSITORY, "GITHUB_REF": catch_up.DEVELOP, "GITHUB_EVENT_NAME": event_name,
                "GITHUB_WORKFLOW_REF": f"{REPOSITORY}/{catch_up.CHECK}@{catch_up.DEVELOP}",
                "GITHUB_EVENT_PATH": str(path), "GH_TOKEN": "fixture"}

    def test_only_completed_develop_staging_runs_or_the_schedule_authorize_a_check(self):
        catch_up.authorized(self.environment())
        catch_up.authorized(self.environment("schedule", {"schedule": "23 * * * *"}))
        refused = [
            self.environment("push"),
            self.environment("workflow_dispatch"),
            dict(self.environment(), GITHUB_REF="refs/heads/main"),
            dict(self.environment(), GITHUB_REPOSITORY="fork/cma"),
            dict(self.environment(), GITHUB_WORKFLOW_REF=f"{REPOSITORY}/.github/workflows/staging-cd.yml@refs/heads/develop"),
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

    def run_main(self, decide=None, rerun=None, env=None):
        output = io.StringIO()
        with patch.dict("os.environ", env or self.environment(), clear=True), \
                patch.object(catch_up, "decide", side_effect=decide), \
                patch.object(catch_up, "rerun", side_effect=rerun), contextlib.redirect_stdout(output):
            status = catch_up.main()
        return status, output.getvalue().splitlines()[-1]

    def test_outcome_line_names_only_the_decision_run_and_lanes(self):
        self.assertEqual(self.run_main(lambda *_: ("nothing-behind", None, [])), (0, "CATCH-UP nothing-behind"))
        run = {"id": 802, "run_attempt": 1}
        self.assertEqual(self.run_main(lambda *_: ("rerun", run, ["am", "runtime-omarchy"]), lambda *_: 2),
                         (0, "CATCH-UP rerun run=802 attempt=2 lanes=am,runtime-omarchy"))

    def test_refusals_fail_with_their_fixed_message_and_unexpected_errors_stay_private(self):
        def unverified(*_args):
            raise Refusal("Catch-up re-run was not verified")
        run = {"id": 802, "run_attempt": 1}
        self.assertEqual(self.run_main(lambda *_: ("rerun", run, ["am"]), unverified),
                         (1, "FAIL catch-up Catch-up re-run was not verified"))
        def private(*_args):
            raise KeyError("private response field")
        self.assertEqual(self.run_main(private), (1, "FAIL catch-up unexpected error"))
        self.assertEqual(self.run_main(env=self.environment("push")),
                         (1, "FAIL catch-up Catch-up requires its develop workflow"))


if __name__ == "__main__":
    unittest.main()
