"""Production selection: release commit proof, version choice and staging evidence."""

import io
import json
import tempfile
import unittest
import zipfile
from pathlib import Path
from unittest.mock import Mock, patch

import prod_cd_select as select
from staging_cd_common import Refusal
from staging_cd_policy import selected, switches
from staging_cd_targets import RUNTIME_TARGETS, TARGETS
from staging_cd_versions import DEPLOYABLES

MERGE, RELEASED, PREVIOUS = "m" * 0 + "1" * 40, "2" * 40, "3" * 40
WEB = "ghcr.io/chronoaiproject/cma-web@sha256:" + "4" * 64


def result(stdout, code=0):
    return Mock(returncode=code, stdout=stdout + "\n")


def git_runner(parents, merge_tree="t" * 40, released_tree="t" * 40):
    def run(args, **_kwargs):
        if args[3:5] == ["rev-list", "--parents"]:
            return result(" ".join(parents))
        if args[3] == "rev-parse":
            return result(merge_tree if args[4].startswith(MERGE) else released_tree)
        return result("", 1)
    return run


class History:
    def __init__(self, order):
        self.positions = {sha: -index for index, sha in enumerate(order)}

    def position(self, sha):
        if sha not in self.positions:
            raise Refusal("Release source is not on verified develop first-parent history")
        return self.positions[sha]


def evidence(rows, extra=None):
    buffer = io.BytesIO()
    with zipfile.ZipFile(buffer, "w") as archive:
        archive.writestr("deployment.json", json.dumps({"schema": select.EVIDENCE_SCHEMA, "components": rows}))
        if extra:
            archive.writestr(extra, "x")
    return buffer.getvalue()


def artifact(identifier, run, *, name=None, expired=False, branch="develop"):
    return {"id": identifier, "name": name or f"staging-deployment-{run}-1", "expired": expired,
            "workflow_run": {"id": run, "head_branch": branch}}


def run_view(run, path=".github/workflows/staging-cd.yml", branch="develop", event="push"):
    return {"id": run, "head_branch": branch, "event": event, "path": path, "conclusion": "success",
            "repository": {"full_name": "ChronoAIProject/cma"}}


class ReleaseCommitTests(unittest.TestCase):
    def test_merge_of_the_qualified_commit_returns_its_second_parent(self):
        runner = git_runner([MERGE, PREVIOUS, RELEASED])
        self.assertEqual(select.release_commit(MERGE, Path("."), runner=runner), RELEASED)

    def test_non_merge_or_different_tree_is_refused(self):
        for runner in (git_runner([MERGE, PREVIOUS]), git_runner([MERGE, PREVIOUS, RELEASED], released_tree="u" * 40)):
            with self.assertRaises(Refusal):
                select.release_commit(MERGE, Path("."), runner=runner)
        with self.assertRaises(Refusal):
            select.release_commit("main", Path("."), runner=git_runner([MERGE, PREVIOUS, RELEASED]))
        with self.assertRaises(Refusal):
            select.release_commit(MERGE, Path("."), runner=lambda *a, **k: result("", 1))


class SelectionTests(unittest.TestCase):
    def test_production_selects_enabled_applications_without_runtime_images(self):
        policy = {"am": False, "eg": False, "web": True}
        self.assertEqual(select.enabled_components(policy, TARGETS, RUNTIME_TARGETS), ["web"])
        policy["am"] = True
        self.assertEqual(select.enabled_components(policy, TARGETS, RUNTIME_TARGETS), ["am", "web"])
        policy["eg"] = True
        self.assertEqual(select.enabled_components(policy, TARGETS, RUNTIME_TARGETS), ["am", "eg", "web"])
        policy["runtime"] = False
        self.assertEqual(select.enabled_components(policy, TARGETS, RUNTIME_TARGETS), ["am", "eg", "web"])

    def test_only_an_explicit_runtime_switch_promotes_runtime_images_and_only_with_am(self):
        policy = {"am": True, "eg": False, "web": True, "runtime": True}
        self.assertEqual(select.enabled_components(policy, TARGETS, RUNTIME_TARGETS),
                         ["am", "runtime", "runtime-omarchy", "web"])
        # Runtime images are Agent Manager settings: the switch alone promotes nothing.
        policy["am"] = False
        self.assertEqual(select.enabled_components(policy, TARGETS, RUNTIME_TARGETS), ["web"])
        # Only a literal true is an explicit switch.
        policy.update(am=True, runtime="enabled")
        self.assertEqual(select.enabled_components(policy, TARGETS, RUNTIME_TARGETS), ["am", "web"])

    def test_staging_still_deploys_runtime_images_with_am_from_the_same_switches(self):
        with tempfile.TemporaryDirectory() as directory:
            script = Path(directory) / "chrono-cd/scripts/deploy_set.py"
            script.parent.mkdir(parents=True)
            config = script.parent.parent / "config.yaml"
            config.write_text("apps:\n  am: {deploy: enabled}\n  eg: {deploy: enabled}\n  web: {deploy: enabled}\n")
            self.assertEqual(select.enabled_components(switches(script), TARGETS, RUNTIME_TARGETS),
                             ["am", "eg", "web"])
            self.assertEqual(selected(script, sorted(TARGETS), automatic=True, mode="release"),
                             ["am", "eg", "runtime", "runtime-omarchy", "web"])
            # A private config cannot carry a runtime switch without a reviewed policy change.
            config.write_text(config.read_text() + "  runtime: {deploy: enabled}\n")
            with self.assertRaisesRegex(Refusal, "target set differs"):
                switches(script)

    def test_newest_version_at_or_before_the_release_commit_is_chosen(self):
        newer, older = "5" * 40, "6" * 40
        history = History([newer, RELEASED, older])
        tags = {"web": [{"name": "cma-web/v0.16.2", "target": newer},
                        {"name": "cma-web/v0.16.1", "target": RELEASED},
                        {"name": "cma-web/v0.16.0", "target": older}]}
        chosen = select.select_versions(RELEASED, {"web": "cma-web"}, tags.__getitem__, history)
        self.assertEqual(chosen, {"web": "0.16.1"})
        with self.assertRaises(Refusal):
            select.select_versions(older, {"web": "cma-web"},
                                   lambda c: [{"name": "cma-web/v0.16.2", "target": newer}], history)


class EvidenceTests(unittest.TestCase):
    def test_evidence_archive_must_hold_exactly_one_bounded_document(self):
        self.assertEqual(select.evidence_rows(evidence({"web": {"state": "controller-verified"}})),
                         {"web": {"state": "controller-verified"}})
        for archive in (evidence({}, extra="other.txt"), b"not a zip"):
            with self.assertRaises(Exception):
                select.evidence_rows(archive)
        buffer = io.BytesIO()
        with zipfile.ZipFile(buffer, "w") as archive:
            archive.writestr("deployment.json", json.dumps({"schema": "other", "components": {}}))
        with self.assertRaises(Refusal):
            select.evidence_rows(buffer.getvalue())

    def test_only_staging_runs_on_develop_are_trusted(self):
        self.assertTrue(select.trusted_run(Mock(return_value=run_view(7)), 7))
        for bad in (run_view(7, path=".github/workflows/ci.yml"), run_view(7, branch="feature"),
                    run_view(7, event="pull_request"), dict(run_view(7), id=8)):
            self.assertFalse(select.trusted_run(Mock(return_value=bad), 7))

    def staging_runs(self, runs, artifacts, workflow="staging-cd.yml"):
        """A request stub: workflow run listings, then each run's own artifacts."""
        def request(path):
            if path.startswith(f"actions/workflows/{workflow}/runs"):
                page = int(path.rsplit("page=", 1)[1])
                return {"workflow_runs": runs[(page - 1) * 100:page * 100]}
            if path.startswith("actions/workflows/"):
                return {"workflow_runs": []}
            if path.startswith("actions/runs/") and "/artifacts" in path:
                return {"artifacts": artifacts.get(int(path.split("/")[2]), [])}
            raise AssertionError(path)
        return request

    def test_verified_image_is_found_through_staging_runs_and_forged_evidence_is_ignored(self):
        runs = [run_view(13, path=".github/workflows/ci.yml"), run_view(12, branch="feature"),
                run_view(14), run_view(15)]
        artifacts = {
            13: [artifact(1, 13)],                                    # untrusted workflow path
            12: [artifact(2, 12)],                                    # not develop
            14: [artifact(3, 14, name="staging-deployment-99-1"),     # names another run
                 artifact(4, 14, expired=True),
                 artifact(5, 14, name="unrelated"),
                 artifact(6, 14)],                                    # trusted, wrong image
            15: [artifact(7, 15, name="staging-deployment-15-2-web")],  # trusted, verified
        }
        archives = {6: evidence({"web": {"state": "controller-verified", "image": WEB.replace("4", "9")}}),
                    7: evidence({"web": {"state": "controller-verified", "image": WEB}})}
        download = Mock(side_effect=lambda item: archives[item["id"]])
        request = self.staging_runs(runs, artifacts)
        self.assertEqual(select.staging_verified({"web": WEB}, request, download), {"web"})
        self.assertEqual([c.args[0]["id"] for c in download.call_args_list], [6, 7])
        archives[7] = evidence({"web": {"state": "rolled-back", "image": WEB}})
        self.assertEqual(select.staging_verified({"web": WEB}, request, download), set())

    def test_old_evidence_beyond_newer_runs_is_still_found(self):
        # Hundreds of newer artifacts elsewhere must not hide an older lane's evidence.
        runs = [run_view(1000 - i) for i in range(150)]
        artifacts = {1000 - i: [artifact(i, 1000 - i, name=f"staging-deployment-{1000 - i}-1-am")]
                     for i in range(149)}
        artifacts[851] = [artifact(9001, 851, name="staging-deployment-851-1-web")]
        download = Mock(return_value=evidence({"web": {"state": "controller-verified", "image": WEB}}))
        self.assertEqual(select.staging_verified({"web": WEB}, self.staging_runs(runs, artifacts), download),
                         {"web"})
        self.assertEqual([c.args[0]["id"] for c in download.call_args_list], [9001])

    def test_contract_and_unsuffixed_evidence_is_always_read(self):
        runs = [run_view(20)]
        artifacts = {20: [artifact(1, 20, name="staging-deployment-20-1-contract"),
                          artifact(2, 20, name="staging-deployment-20-1"),
                          artifact(3, 20, name="staging-deployment-20-01-web")]}
        download = Mock(return_value=evidence({}))
        select.staging_verified({"web": WEB}, self.staging_runs(runs, artifacts), download)
        self.assertEqual([c.args[0]["id"] for c in download.call_args_list], [1, 2])

    def test_automatic_retry_evidence_counts_for_its_own_lane_only(self):
        runs = [run_view(30)]
        artifacts = {30: [artifact(1, 30, name="staging-deployment-30-1-am-retry"),
                          artifact(2, 30, name="staging-deployment-30-1-web"),
                          artifact(3, 30, name="staging-deployment-30-1-web-retry")]}
        download = Mock(return_value=evidence({"web": {"state": "controller-verified", "image": WEB}}))
        self.assertEqual(select.staging_verified({"web": WEB}, self.staging_runs(runs, artifacts), download),
                         {"web"})
        # The AM retry archive is another component's lane and is never downloaded.
        self.assertEqual([c.args[0]["id"] for c in download.call_args_list], [2])
        self.assertEqual(select.evidence_lane("staging-deployment-30-1-runtime-omarchy-retry", 30),
                         "runtime-omarchy")
        self.assertEqual(select.evidence_lane("staging-deployment-30-1-am", 30), "am")

    def two_workflow_runs(self, staging, redeploy, artifacts):
        """A request stub listing staging-cd and redeploy-staging runs separately."""
        listings = {"staging-cd.yml": staging, "redeploy-staging.yml": redeploy}
        order = []

        def request(path):
            if path.startswith("actions/workflows/"):
                name = path.split("/")[2]
                page = int(path.rsplit("page=", 1)[1])
                return {"workflow_runs": listings[name][(page - 1) * 100:page * 100]}
            if path.startswith("actions/runs/") and "/artifacts" in path:
                order.append(int(path.split("/")[2]))
                return {"artifacts": artifacts.get(int(path.split("/")[2]), [])}
            raise AssertionError(path)
        return request, order

    def test_redeploy_evidence_is_not_starved_behind_staging_runs(self):
        # Run 45290771313's AM was recovered by redeploy-staging 45297741449. Its
        # evidence sat behind every staging CD run until the listing bound refused.
        # Staging CD runs carry even ids; the one redeploy run is odd, between them.
        staging = [run_view(10_000 - 2 * i) for i in range(select.MAX_RUN_LISTINGS + 50)]
        redeploy = [run_view(9_991, path=".github/workflows/redeploy-staging.yml", event="workflow_dispatch")]
        artifacts = {9_991: [artifact(1, 9_991, name="staging-deployment-9991-1-am")]}
        request, order = self.two_workflow_runs(staging, redeploy, artifacts)
        download = Mock(return_value=evidence({"am": {"state": "controller-verified", "image": WEB}}))
        self.assertEqual(select.staging_verified({"am": WEB}, request, download), {"am"})
        # Newest first across both workflows: five newer staging runs, then the redeploy.
        self.assertEqual(order, [10_000 - 2 * i for i in range(5)] + [9_991])

    def test_a_redeploy_run_listed_under_the_other_workflow_is_not_trusted(self):
        staging = [run_view(20)]
        redeploy = [run_view(21)]  # claims the staging-cd path from the redeploy listing
        artifacts = {21: [artifact(1, 21, name="staging-deployment-21-1-am")]}
        request, order = self.two_workflow_runs(staging, redeploy, artifacts)
        download = Mock(return_value=evidence({"am": {"state": "controller-verified", "image": WEB}}))
        self.assertEqual(select.staging_verified({"am": WEB}, request, download), set())
        self.assertEqual(order, [20])

    def test_evidence_search_is_bounded(self):
        runs = [run_view(5000 - i) for i in range(select.MAX_RUN_LISTINGS + 1)]
        with self.assertRaises(Refusal):
            select.staging_verified({"web": WEB}, self.staging_runs(runs, {}), Mock(return_value=evidence({})))
        runs = [run_view(10)]
        artifacts = {10: [artifact(i, 10, name=f"staging-deployment-10-{i + 1}") for i in range(select.MAX_DOWNLOADS + 1)]}
        with self.assertRaises(Refusal):
            select.staging_verified({"web": WEB}, self.staging_runs(runs, artifacts), Mock(return_value=evidence({})))


class PlanTests(unittest.TestCase):
    def environment(self, root):
        event = root / "event.json"
        event.write_text(json.dumps({"ref": "refs/heads/main", "after": MERGE, "deleted": False,
                                     "repository": {"full_name": "ChronoAIProject/cma"}}))
        return {"CD_TARGET": "production", "GITHUB_REPOSITORY": "ChronoAIProject/cma",
                "GITHUB_REF": "refs/heads/main", "GITHUB_EVENT_NAME": "push", "GITHUB_SHA": MERGE,
                "GITHUB_EVENT_PATH": str(event)}

    def test_plan_selects_promotes_and_requires_staging_evidence(self):
        with tempfile.TemporaryDirectory() as directory:
            env = self.environment(Path(directory))
            api = Mock(tags=lambda c: [{"name": "cma-web/v0.16.1", "target": RELEASED}],
                       history=Mock(return_value=History([RELEASED])))
            receipt = {"repository": WEB.split("@")[0], "digest": WEB.split("@")[1]}
            value = {"schema": "cma-deployment-set-v1", "receipts": {"web": receipt}}
            policy = {"am": False, "eg": False, "web": True}
            runner = git_runner([MERGE, PREVIOUS, RELEASED])
            with patch("staging_cd_set.dispatch_set", return_value=("release", value)) as dispatch, \
                    patch.object(select, "staging_verified", return_value={"web"}):
                self.assertEqual(select.plan(env, api, Mock(), policy, root=Path(directory), download=Mock(),
                                             deployables={"web": "cma-web"}, runner=runner), (value, RELEASED))
            inputs = dispatch.call_args.args[0]
            self.assertEqual((inputs["deployables"], json.loads(inputs["versions"]), inputs["mode"]),
                             ("web", {"web": "0.16.1"}, "release"))
            with patch("staging_cd_set.dispatch_set", return_value=("release", value)), \
                    patch.object(select, "staging_verified", return_value=set()), \
                    self.assertRaisesRegex(Refusal, "never verified"):
                select.plan(env, api, Mock(), policy, root=Path(directory), download=Mock(),
                            deployables={"web": "cma-web"}, runner=runner)

    def test_plan_refuses_other_targets_and_skips_when_nothing_is_enabled(self):
        with tempfile.TemporaryDirectory() as directory:
            env = self.environment(Path(directory))
            runner = git_runner([MERGE, PREVIOUS, RELEASED])
            off = {"am": False, "eg": False, "web": False}
            self.assertEqual(select.plan(env, Mock(), Mock(), off, root=Path(directory), download=Mock(),
                                         deployables={}, runner=runner), (None, RELEASED))
            with self.assertRaises(Refusal):
                select.plan(dict(env, CD_TARGET="staging"), Mock(), Mock(), off, root=Path(directory),
                            download=Mock(), deployables={}, runner=runner)

    def test_a_dispatch_retry_requires_maintainer_actors(self):
        with tempfile.TemporaryDirectory() as directory:
            env = self.environment(Path(directory))
            Path(env["GITHUB_EVENT_PATH"]).write_text(json.dumps({"ref": "refs/heads/main", "inputs": {"infra_ref": ""},
                                                                  "repository": {"full_name": "ChronoAIProject/cma"}}))
            env.update(GITHUB_EVENT_NAME="workflow_dispatch", GITHUB_ACTOR="fixture-author",
                       GITHUB_TRIGGERING_ACTOR="fixture-maintainer")
            runner = git_runner([MERGE, PREVIOUS, RELEASED])
            off = {"am": False, "eg": False, "web": False}
            api = Mock(request=Mock(return_value={"role_name": "maintain"}))
            self.assertEqual(select.plan(env, api, Mock(), off, root=Path(directory), download=Mock(),
                                         deployables={}, runner=runner), (None, RELEASED))
            self.assertEqual([call.args[0] for call in api.request.call_args_list],
                             ["collaborators/fixture-author/permission", "collaborators/fixture-maintainer/permission"])
            api.request.side_effect = [{"role_name": "admin"}, {"role_name": "write"}]
            with self.assertRaisesRegex(Refusal, "maintainer permission"):
                select.plan(env, api, Mock(), off, root=Path(directory), download=Mock(), deployables={},
                            runner=runner)
            # A push needs no actor permission read.
            api = Mock()
            select.plan(self.environment(Path(directory)), api, Mock(), off, root=Path(directory),
                        download=Mock(), deployables={}, runner=runner)
            api.request.assert_not_called()



class EntryTests(unittest.TestCase):
    def test_download_streams_one_artifact_through_gh_and_refuses_failures(self):
        def fake_run(args, stdout, **_kwargs):
            stdout.write(b"zip-bytes")
            return Mock(returncode=0)
        with patch.object(select.subprocess, "run", side_effect=fake_run) as run:
            self.assertEqual(select.gh_download("token")({"id": 5}), b"zip-bytes")
            self.assertEqual(run.call_args.args[0][-1], "repos/ChronoAIProject/cma/actions/artifacts/5/zip")
            self.assertEqual(run.call_args.kwargs["env"]["GH_TOKEN"], "token")
        with patch.object(select.subprocess, "run", return_value=Mock(returncode=1)), self.assertRaises(Refusal):
            select.gh_download("token")({"id": 5})
        with self.assertRaises(Refusal):
            select.gh_download("token")({"id": "5"})

    def test_main_plans_then_reproduces_the_approved_set_before_starting_the_clock(self):
        outputs = {}
        env = {"CD_PRIVATE_CONTROLLER": "/private/prod/chrono-cd", "GH_TOKEN": "token", "CD_TARGET": "production"}
        value = {"schema": "cma-deployment-set-v1", "receipts": {}}
        with patch.dict(select.os.environ, env, clear=True), \
                patch("staging_cd_runtime.run_private", autospec=True,
                      side_effect=lambda operation, _, **_kw: 0 if operation(None) else 1) as boundary, \
                patch("staging_cd_resolve.output", side_effect=lambda _env, name, v: outputs.__setitem__(name, v)), \
                patch("staging_cd_github.GitHub"), patch("staging_cd_registry.Registry"), \
                patch("staging_cd_policy.switches", return_value={"web": True}) as switches, \
                patch("staging_cd_clock.timestamp", return_value="2026-11-20T00:00:00Z"):
            with patch.object(select, "plan", return_value=(value, RELEASED)) as planner:
                self.assertEqual(select.main(), 0)
                # autospec holds the call to the real boundary signature.
                self.assertEqual(boundary.call_args.kwargs["subject"], "production-release")
                self.assertEqual(switches.call_args.args[0], Path("/private/prod/chrono-cd/scripts/deploy_set.py"))
                self.assertEqual(planner.call_args.args[3], {"web": True})
                self.assertEqual((outputs["released"], outputs["mode"]), (RELEASED, "release"))
                self.assertNotIn("started_at", outputs)
                self.assertEqual(json.loads(outputs["release_set"]), value)
                outputs.clear()
                select.os.environ["CD_RELEASE_SET"] = json.dumps(value)
                switches.reset_mock()
                self.assertEqual(select.main(), 0)
                self.assertEqual(outputs["started_at"], "2026-11-20T00:00:00Z")
                # After approval the components come from the approved set, not the private config.
                switches.assert_not_called()
                self.assertEqual(planner.call_args.args[3], {"am": False, "eg": False, "web": False})
                # An approved runtime image is refused before any re-selection, never dropped.
                planner.reset_mock()
                for runtime in sorted(RUNTIME_TARGETS):
                    select.os.environ["CD_RELEASE_SET"] = json.dumps(
                        dict(value, receipts={"am": {}, "eg": {}, "web": {}, runtime: {}}))
                    with self.assertRaisesRegex(Refusal, "must not hold a runtime image"):
                        select.main()
                planner.assert_not_called()
                select.os.environ["CD_RELEASE_SET"] = json.dumps(dict(value, receipts={"web": {}}))
                with self.assertRaisesRegex(Refusal, "no longer matches"):
                    select.main()
                del select.os.environ["CD_RELEASE_SET"]
            outputs.clear()
            with patch.object(select, "plan", return_value=(None, RELEASED)):
                self.assertEqual(select.main(), 0)
            self.assertEqual(outputs, {"released": RELEASED, "release_set": ""})

    def test_an_enabled_am_release_selects_and_reproduces_applications_without_runtimes(self):
        outputs, plan, runner = {}, select.plan, git_runner([MERGE, PREVIOUS, RELEASED])
        api = Mock(tags=lambda c: [{"name": DEPLOYABLES[c] + "/v1.2.3", "target": RELEASED}],
                   history=Mock(return_value=History([RELEASED])))

        def dispatch(inputs, _api, _registry):
            return "release", {"schema": "cma-deployment-set-v1", "receipts": {
                c: {"repository": "ghcr.io/chronoaiproject/" + DEPLOYABLES[c], "digest": "sha256:" + "4" * 64}
                for c in inputs["deployables"].split(",")}}

        with tempfile.TemporaryDirectory() as directory:
            env = dict(PlanTests().environment(Path(directory)), GH_TOKEN="token",
                       CD_PRIVATE_CONTROLLER="/private/prod/chrono-cd")
            with patch.dict(select.os.environ, env, clear=True), \
                    patch("staging_cd_runtime.run_private", autospec=True,
                          side_effect=lambda operation, _, **_kw: 0 if operation(None) else 1), \
                    patch("staging_cd_resolve.output", side_effect=lambda _env, name, v: outputs.__setitem__(name, v)), \
                    patch("staging_cd_github.GitHub", return_value=api), patch("staging_cd_registry.Registry"), \
                    patch("staging_cd_policy.switches", return_value={"am": True, "eg": True, "web": True}), \
                    patch("staging_cd_clock.timestamp", return_value="2026-11-20T00:00:00Z"), \
                    patch("staging_cd_set.dispatch_set", side_effect=dispatch) as dispatcher, \
                    patch.object(select, "staging_verified", side_effect=lambda images, *_: set(images)), \
                    patch.object(select, "plan", side_effect=lambda *a, **k: plan(*a, runner=runner, **k)):
                self.assertEqual(select.main(), 0)
                planned = dispatcher.call_args.args[0]
                self.assertEqual((planned["deployables"], sorted(json.loads(planned["versions"]))),
                                 ("am,eg,web", ["am", "eg", "web"]))
                self.assertEqual(sorted(json.loads(outputs["release_set"])["receipts"]), ["am", "eg", "web"])
                # The deploy job re-derives the policy from the approved set and reproduces it exactly.
                select.os.environ["CD_RELEASE_SET"] = outputs["release_set"]
                self.assertEqual(select.main(), 0)
                self.assertEqual(dispatcher.call_args.args[0], planned)
                self.assertEqual(outputs["started_at"], "2026-11-20T00:00:00Z")

if __name__ == "__main__":
    unittest.main()
