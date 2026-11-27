"""Production target: the shared CD helpers act for main and prod/chrono-cd only when asked."""

import json
import tempfile
import unittest
from pathlib import Path
from unittest.mock import Mock, patch

import cd_target
import staging_cd_invoke as invoke
import staging_cd_invoke_tests as invoke_fixtures
from staging_cd_admission import event_identity
from staging_cd_checkout import checkout_path
from staging_cd_clock import CONTROLLER_TOTAL_ENV, dispatch_started, run_created
from staging_cd_common import Refusal
from staging_cd_deploy_result_tests import fixture_result
from staging_cd_receipts import canonical
from staging_cd_receipts_tests import fixture_receipt, fixture_runtime_receipt
from staging_cd_set import release_set
from staging_cd_targets import RUNTIME_TARGETS
from staging_cd_tools import install_requirements

SHA = "a" * 40
INFRA = "c" * 40
DECLARED = object()  # the inputs the dispatch workflow declares, at their defaults


def timestamp(seconds):
    from datetime import datetime, timezone

    return datetime.fromtimestamp(seconds, timezone.utc).strftime("%Y-%m-%dT%H:%M:%SZ")


def environment(root, branch, target=None, event_name="push", inputs=DECLARED):
    event = root / "event.json"
    value = {"ref": "refs/heads/" + branch, "after": SHA, "deleted": False,
             "repository": {"full_name": "ChronoAIProject/cma"}}
    if event_name == "workflow_dispatch":
        # The inputs each dispatch workflow declares: prod-cd.yml or redeploy-staging.yml.
        if inputs is DECLARED:
            inputs = ({"infra_ref": ""} if target == "production"
                      else {"deployables": "web", "version": "", "versions": "", "mode": "release", "infra_ref": ""})
        value = {"ref": "refs/heads/" + branch, "inputs": inputs,
                 "repository": {"full_name": "ChronoAIProject/cma"}}
    event.write_text(json.dumps(value))
    env = {"GITHUB_REPOSITORY": "ChronoAIProject/cma", "GITHUB_REF": "refs/heads/" + branch,
           "GITHUB_EVENT_NAME": event_name, "GITHUB_SHA": SHA, "GITHUB_EVENT_PATH": str(event),
           "GITHUB_WORKSPACE": str(root), "RUNNER_TEMP": str(root), "GITHUB_RUN_ID": "123",
           "GITHUB_RUN_ATTEMPT": "1", "CD_INFRA_REF": INFRA}
    if target is not None:
        env["CD_TARGET"] = target
    return env


class TargetTests(unittest.TestCase):
    def test_absent_target_is_staging_and_unknown_target_is_refused(self):
        self.assertEqual(cd_target.branch({}), "develop")
        self.assertEqual(cd_target.controller({}), ("staging",))
        self.assertEqual(cd_target.workflows({})["push"], "staging-cd.yml")
        self.assertEqual((cd_target.name({}), cd_target.name({"CD_TARGET": "production"})), ("staging", "production"))
        for lookup in (cd_target.current, cd_target.name):
            with self.subTest(lookup=lookup.__name__), self.assertRaises(Refusal):
                lookup({"CD_TARGET": "prod"})

    def test_production_target_is_main_and_the_production_controller(self):
        env = {"CD_TARGET": "production"}
        self.assertEqual(cd_target.branch(env), "main")
        self.assertEqual(cd_target.controller(env), ("prod", "chrono-cd"))
        self.assertEqual(cd_target.workflows(env),
                         {"push": "prod-cd.yml", "workflow_dispatch": "prod-cd.yml"})

    def test_workflow_map_is_a_copy(self):
        cd_target.workflows({})["push"] = "changed.yml"
        self.assertEqual(cd_target.workflows({})["push"], "staging-cd.yml")


class EventIdentityTests(unittest.TestCase):
    def test_each_target_accepts_only_its_own_branch(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            event_identity(environment(root, "main", "production"))
            event_identity(environment(root, "develop"))
            event_identity(environment(root, "main", "production", "workflow_dispatch"))
            for env in (environment(root, "develop", "production"), environment(root, "main")):
                with self.assertRaises(Refusal):
                    event_identity(env)

    def test_a_prod_cd_dispatch_is_admitted_with_only_the_inputs_it_declares(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            # Empty resolves cma-infra main at run start; a pinned commit or main is explicit.
            for inputs in ({"infra_ref": ""}, {"infra_ref": INFRA}, {"infra_ref": "main"}, {}):
                with self.subTest(inputs=inputs):
                    event = event_identity(environment(root, "main", "production", "workflow_dispatch", inputs))
                    self.assertEqual(event["inputs"], inputs)
            for inputs in ({"deployables": "web"}, {"infra_ref": "", "deployables": "web"},
                           {"infra_ref": "", "mode": "bootstrap"}, {"infra_ref": None}, {"infra_ref": 5},
                           {"infra_ref": "feature"}, {"infra_ref": "C" * 40}, None, []):
                with self.subTest(inputs=inputs), self.assertRaisesRegex(Refusal, "dispatch inputs are invalid"):
                    event_identity(environment(root, "main", "production", "workflow_dispatch", inputs))

    def test_a_staging_dispatch_still_requires_its_deployables(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            event_identity(environment(root, "develop", event_name="workflow_dispatch"))
            event_identity(environment(root, "develop", event_name="workflow_dispatch",
                                       inputs={"deployables": "web", "unexpected": "kept"}))
            for inputs in ({"infra_ref": ""}, {"deployables": 5}):
                with self.subTest(inputs=inputs), self.assertRaisesRegex(Refusal, "dispatch inputs are invalid"):
                    event_identity(environment(root, "develop", event_name="workflow_dispatch", inputs=inputs))


class ClockTests(unittest.TestCase):
    def test_production_admission_clock_trusts_only_prod_cd_runs_on_main(self):
        env = {"GITHUB_RUN_ID": "123", "GITHUB_SHA": SHA, "GITHUB_RUN_ATTEMPT": "1",
               "GITHUB_EVENT_NAME": "workflow_dispatch", "CD_TARGET": "production"}
        run = {"id": 123, "event": "workflow_dispatch", "head_branch": "main", "head_sha": SHA,
               "path": ".github/workflows/prod-cd.yml", "created_at": timestamp(1000),
               "repository": {"full_name": "ChronoAIProject/cma"}}
        with patch("staging_cd_clock.time.time", return_value=1020):
            self.assertEqual(dispatch_started(env, Mock(return_value=run)), timestamp(1000))
            self.assertEqual(run_created(dict(env, GITHUB_EVENT_NAME="push"),
                                         Mock(return_value=dict(run, event="push"))), timestamp(1000))
            for key, value in (("head_branch", "develop"), ("path", ".github/workflows/redeploy-staging.yml")):
                with self.subTest(key=key), self.assertRaises(Refusal):
                    dispatch_started(env, Mock(return_value=dict(run, **{key: value})))


class BudgetTests(unittest.TestCase):
    """The coordinator and its supervising launcher share one absolute deadline."""

    def test_the_launcher_reserves_each_targets_own_controller_total(self):
        with tempfile.TemporaryDirectory() as directory:
            script = Path(directory) / "prod/chrono-cd/scripts/deploy_set.py"
            for target, mode, total in ((None, "bootstrap", 600), (None, "release", 990), ("staging", "release", 990),
                                        ("production", "bootstrap", 600), ("production", "release", 3600)):
                options = {} if target is None else {"target": target}
                with self.subTest(target=target, mode=mode), patch("staging_cd_clock.time.time", return_value=1010), \
                        patch.object(invoke, "run_process", return_value=0) as process:
                    self.assertEqual(invoke.execute(script, Path(directory) / "input", mode, timestamp(1000), {},
                                                    **options), 0)
                    self.assertEqual(process.call_args.args[1][CONTROLLER_TOTAL_ENV], str(total))
                    self.assertEqual(process.call_args.args[2], total - 10)
                    self.assertEqual(process.call_args.kwargs["grace_seconds"], 15)

    def deploy(self, target, finish, *, components=("web",), mode="release", profile=None, withheld=()):
        """One release through the default launcher; the controller returns at `finish`."""
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            env = invoke_fixtures.InvokeTests().environment(root)
            receipts = {key: (fixture_runtime_receipt if key in RUNTIME_TARGETS else fixture_receipt)(
                component=key) for key in components}
            env.update(CD_STARTED_AT=timestamp(1000), CD_DEPLOY_MODE=mode,
                       CD_RELEASE_SET=canonical(release_set(receipts)))
            if target is not None:
                env.update(environment(root, "main", target))
            elif mode == "bootstrap":
                env.update(environment(root, "develop", event_name="workflow_dispatch",
                                       inputs={"deployables": "am,eg,web", "mode": mode}))
            script = root / "controller/scripts/deploy_set.py"
            script.parent.mkdir(parents=True)
            marker = script.with_name("am_rollout_budget.py")
            if profile == "supported":
                marker.touch()
            elif profile == "symlink":
                script.touch()
                marker.symlink_to(script)
            clock, launched = [1010], []

            def process(args, child_env, wait, **kwargs):
                launched.append((child_env[CONTROLLER_TOTAL_ENV], wait, kwargs["grace_seconds"]))
                fixture_result(Path(args[args.index("--release-set") + 1]))
                clock[0] = finish
                return 0

            with patch("staging_cd_clock.time.time", side_effect=lambda: clock[0]), \
                    patch.object(invoke, "run_process", side_effect=process), \
                    patch.object(invoke, "controller_receipts",
                                 side_effect=lambda receipts, *_args: (
                                     {key: value for key, value in receipts.items() if key not in withheld}, 0)):
                code = invoke.deploy(env, Mock(), lambda _env: script,
                                     policy_reader=lambda _script, selected, **_kw: sorted(selected))
            return code, launched

    def test_a_production_release_outlasts_the_staging_total_and_staging_keeps_it(self):
        self.assertEqual(self.deploy(None, 1985), (0, [("990", 980, 15)]))
        self.assertEqual(self.deploy(None, 1991), (20, [("990", 980, 15)]))
        for finish in (1991, 3000, 4600):
            with self.subTest(finish=finish):
                self.assertEqual(self.deploy("production", finish), (0, [("3600", 3590, 15)]))
        # A result after the production total is still unverified.
        self.assertEqual(self.deploy("production", 4601), (20, [("3600", 3590, 15)]))

    def test_am_budget_requires_a_supported_staging_release_that_actually_affects_am(self):
        for component in ("am", "runtime", "runtime-omarchy"):
            with self.subTest(component=component):
                self.assertEqual(self.deploy(None, 3700, components=(component,), profile="supported"),
                                 (0, [("2700", 2690, 15)]))
                self.assertEqual(self.deploy(None, 3701, components=(component,), profile="supported"),
                                 (20, [("2700", 2690, 15)]))
        for profile in (None, "symlink"):
            self.assertEqual(self.deploy(None, 1991, components=("am",), profile=profile),
                             (20, [("990", 980, 15)]))
        self.assertEqual(self.deploy(None, 1991, components=("am", "web"), profile="supported", withheld=("am",)),
                         (20, [("990", 980, 15)]))
        self.assertEqual(self.deploy(None, 1991, profile="supported"), (20, [("990", 980, 15)]))
        self.assertEqual(self.deploy("production", 3000, components=("am",), profile="supported"),
                         (0, [("3600", 3590, 15)]))
        self.assertEqual(self.deploy(None, 1600, components=("am", "eg", "web"), mode="bootstrap", profile="supported"),
                         (0, [("600", 590, 15)]))


class ControllerPathTests(unittest.TestCase):
    def test_production_runs_only_the_production_entry_point_at_the_reviewed_commit(self):
        with tempfile.TemporaryDirectory() as directory:
            env = environment(Path(directory), "main", "production")
            root = checkout_path(env)
            staging = root / "staging/scripts/deploy_set.py"
            staging.parent.mkdir(parents=True)
            staging.write_text("print('staging')\n")
            runner = Mock(side_effect=[Mock(returncode=0, stdout=INFRA + "\n"), Mock(returncode=0, stdout="")])
            with self.assertRaises(Refusal):
                invoke.private_script(env, runner)
            script = root / "prod/chrono-cd/scripts/deploy_set.py"
            script.parent.mkdir(parents=True)
            script.write_text("print('production')\n")
            runner = Mock(side_effect=[Mock(returncode=0, stdout=INFRA + "\n"), Mock(returncode=0, stdout="")])
            self.assertEqual(invoke.private_script(env, runner), script)
            self.assertEqual(runner.call_args_list[1].args[0][-1], "prod/chrono-cd")

    def test_production_entry_point_links_are_refused(self):
        with tempfile.TemporaryDirectory() as directory:
            env = environment(Path(directory), "main", "production")
            root = checkout_path(env)
            real = Path(directory) / "elsewhere"
            (real / "scripts").mkdir(parents=True)
            (real / "scripts/deploy_set.py").write_text("print('elsewhere')\n")
            (root / "prod").mkdir(parents=True)
            (root / "prod/chrono-cd").symlink_to(real)
            with self.assertRaises(Refusal):
                invoke.private_script(env, Mock())

    def test_production_reads_the_production_controller_requirements(self):
        with tempfile.TemporaryDirectory() as directory:
            env = environment(Path(directory), "main", "production")
            root = checkout_path(env)
            (root / "staging").mkdir(parents=True)
            (root / "staging/requirements.txt").write_text("PyYAML==6.0.3\n")
            with self.assertRaisesRegex(Refusal, "requirements are unavailable"):
                install_requirements(env, Mock())
            (root / "prod/chrono-cd").mkdir(parents=True)
            (root / "prod/chrono-cd/requirements.txt").write_text("unreviewed==1.0\n")
            # Reaching the pin review proves the production file was the one read.
            with self.assertRaisesRegex(Refusal, "pins require review"):
                install_requirements(env, Mock())


if __name__ == "__main__":
    unittest.main()
