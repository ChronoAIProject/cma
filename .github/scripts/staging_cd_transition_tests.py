"""A later AM change owns any still-outstanding peers of a contract transition."""
from contextlib import chdir
import copy
import json
from pathlib import Path
import subprocess
import tempfile
import unittest
from unittest.mock import Mock, patch

import yaml

from staging_cd_allocate import allocate_queue
from staging_cd_applied import configured_group
from staging_cd_allocation_fixture import AllocationServer
from staging_cd_common import Refusal
from staging_cd_compatibility import ContractPolicy, GROUP, RECIPE, VERSION_FILE
from staging_cd_component_admission import automatic_admission
from staging_cd_contract_tests import fixture_source
from staging_cd_deploy_result_tests import fixture_result
from staging_cd_history import DevelopHistory
from staging_cd_invoke import CONTRACT_READ_FAILED, NotAdmitted, UNVERIFIED_COHORT, deploy
import staging_cd_invoke_tests as invoke_fixtures
from staging_cd_queue import release_queue
from staging_cd_promotion import proof
from staging_cd_receipts import canonical
from staging_cd_receipts_tests import fixture_receipt, fixture_runtime_receipt, fixture_tag
from staging_cd_routes import check_lane, routes
from staging_cd_set import private_document, verify_set
from staging_cd_transition import transition_in_window
from staging_cd_versions import DEPLOYABLES
from staging_cd_watermark import SupersededVersions


class TransitionTests(unittest.TestCase):
    def setUp(self):
        directory = tempfile.TemporaryDirectory()
        self.addCleanup(directory.cleanup)
        self.root = Path(directory.name)
        def git(*args):
            return subprocess.run(["git", "-c", "user.name=Test", "-c", "user.email=test@example.invalid",
                                   "-c", "commit.gpgSign=false", *args], cwd=self.root, check=True, capture_output=True,
                                  text=True, timeout=10).stdout.strip()
        self.git = git
        git("init", "--quiet")
        fixture_source(self.root)
        current = {key: (self.root / key).read_text() for key in (RECIPE, VERSION_FILE)}
        for path, content in current.items():
            (self.root / path).write_text(content.replace('"3"', '"2"'))
        git("add", ".")
        git("commit", "-qm", "Old contract")
        base = git("rev-parse", "HEAD")
        for path, content in current.items():
            (self.root / path).write_text(content)
        git("commit", "-qam", "Transition")
        self.first = git("rev-parse", "HEAD")
        git("commit", "--allow-empty", "-qm", "AM successor")
        self.head = git("rev-parse", "HEAD")
        self.history = DevelopHistory(self.head, [self.head, self.first, base])
        self.policy = ContractPolicy(self.history, root=self.root)
        self.rows = [{"source": self.first, "tree": git("rev-parse", self.first + "^{tree}"),
                      "deployables": sorted(DEPLOYABLES[k] for k in GROUP | {"eg"})},
                     {"source": self.head, "tree": git("rev-parse", self.head + "^{tree}"),
                      "deployables": [DEPLOYABLES["am"]]}]
        self.anchor = transition_in_window(self.root, self.history, self.rows)
        self.assertEqual(self.anchor, self.first)
        self.receipts = {k: (fixture_runtime_receipt if k.startswith("runtime") else fixture_receipt)(
            component=k, source=self.first, tree=self.rows[0]["tree"]) for k in GROUP | {"eg"}}
        self.tags = [fixture_tag(r) for r in self.receipts.values()]
        server = AllocationServer(1, reserved=False)
        server.sources = [self.first, self.head]
        self.api, self.registry, self.store = Mock(), Mock(), Mock()
        self.api.tags.side_effect = lambda key=None: [t for t in self.tags if key in (None, t["receipt"]["identity"]["component"])]
        self.api.request = server.request
        self.api.history.return_value = self.history
        self.store.list.return_value = []
        self.store.insert.return_value = True
        self.registry.exists.return_value = True
        self.registry.manifest.side_effect = lambda repo, version: next(
            (t["receipt"]["digest"], {}) for t in self.tags if t["receipt"]["repository"] == repo
            and t["receipt"]["identity"]["version"] == version)

    def allocate(self, head, changes, *, proofs=None):
        return allocate_queue({"head": head, "event_source": head, "changes": changes,
                               "contract_transition": self.anchor}, self.api, self.store, 999, 1, read_proofs=lambda: proofs or {})

    def successor(self):
        matrix = self.allocate(self.head, self.rows)
        self.assertEqual({e["component"] for e in matrix["include"]}, GROUP | {"eg"})
        for entry in matrix["include"]:
            task, = json.loads(entry["tasks"])
            if entry["component"] == "am":
                self.assertEqual(task["mode"], "publish")
                identity = {k: v for k, v in task["reservation"].items() if k != "schema"}
                receipt = fixture_receipt(**identity)
                self.tags.append(fixture_tag(receipt))
            else:
                self.assertEqual(task["receipt"], self.receipts[entry["component"]])
        lanes, batches = routes(matrix, lambda: self.policy)
        self.assertEqual(lanes, {**dict.fromkeys(GROUP, "contract"), "eg": "eg"})
        gateway, failures = automatic_admission(batches["eg"], self.api, self.registry, policy=self.policy)
        self.assertFalse(failures)
        self.assertEqual(gateway["receipts"], {"eg": self.receipts["eg"]})
        return batches["contract"]

    def test_old_run_keeps_gateway_and_successor_submits_complete_current_group(self):
        old = self.allocate(self.first, self.rows[:1])
        admitted, errors = automatic_admission(old, self.api, self.registry, policy=self.policy)
        self.assertFalse(errors)
        matrix = self.successor()
        self.api.require_current_versions.side_effect = SupersededVersions({"am"})
        with patch("staging_cd_set.ContractPolicy", return_value=self.policy):
            old_locked = verify_set(admitted, self.api, self.registry, automatic=True)
        self.assertEqual(set(old_locked["receipts"]), {"eg"})
        self.api.require_current_versions.side_effect = None
        selected, errors = automatic_admission(matrix, self.api, self.registry, policy=self.policy)
        self.assertFalse(errors)
        with patch("staging_cd_set.ContractPolicy", return_value=self.policy):
            locked = verify_set(selected, self.api, self.registry, automatic=True)
        self.assertEqual(locked["contract_transition"], self.first)
        for configured, code in [(set(), 0), ({"runtime", "runtime-omarchy"}, 0), (GROUP, UNVERIFIED_COHORT)]:
            with self.subTest(configured=configured), tempfile.TemporaryDirectory() as directory:
                root = Path(directory)
                env = invoke_fixtures.InvokeTests().environment(root)
                env["CD_RELEASE_SET"] = canonical(locked)
                def controller(_script, path, *_args):
                    result = fixture_result(path)
                    self.assertEqual(set(result["components"]), GROUP)
                    for key, row in result["components"].items():
                        self.assertEqual(row["source_sha"], self.head if key == "am" else self.first)
                    return 0
                runner = Mock(side_effect=controller)
                with patch("staging_cd_invoke.configured_group", return_value=configured), \
                        patch("staging_cd_invoke.require_current_versions"):
                    self.assertEqual(deploy(env, Mock(), lambda _: root / "controller.py", runner,
                                            policy_reader=lambda _script, keys, **_kw: sorted(keys)), code)
                self.assertEqual(runner.call_count, int(code == 0))
        # At the current head, request-bound proofs for both lanes close all outstanding work.
        proofs = {}
        for release in (old_locked, locked):
            stamp = proof(private_document(release)["components"])
            proofs.update({key: {"writer": self.head, "stamp": stamp} for key in release["receipts"]})
        self.assertEqual(self.allocate(self.head, self.rows, proofs=proofs), {"include": []})
        self.assertEqual(self.allocate("d" * 40, self.rows), {"include": []})

    def test_cached_comparisons_use_the_current_adoption_and_transition(self):
        decisions = {row["source"]: {"deployables": row["deployables"]} for row in self.rows}
        with patch("staging_cd_queue.batch_selection", return_value=decisions) as compare:
            full = release_queue(self.root, self.first, self.head, self.head, {})
            self.assertEqual(full["contract_transition"], self.first)
            self.assertEqual(full["changes"], self.rows)
            restricted = release_queue(self.root, self.head, self.head, self.head, {})
            self.assertNotIn("contract_transition", restricted)
            self.assertEqual(restricted["changes"], self.rows[1:])
            with self.assertRaisesRegex(Refusal, "no longer on develop"):
                release_queue(self.root, self.first, self.head, "f" * 40, {})
            self.assertEqual(compare.call_count, 2)

    def test_routing_preserves_a_transition_older_than_every_selected_source(self):
        # Both runtimes finalize after the transition. A later AM and gateway
        # now have work while the original three-member rollout is outstanding.
        self.git("commit", "--allow-empty", "-qm", "Independent application changes")
        newest = self.git("rev-parse", "HEAD")
        history = DevelopHistory(newest, self.git("rev-list", "--first-parent", newest).splitlines())
        policy = ContractPolicy(history, root=self.root)
        self.api.history.return_value = history
        runtime_source = self.head
        tree = self.rows[0]["tree"]
        changes = [self.rows[0],
                   {"source": runtime_source, "tree": tree, "deployables": sorted(DEPLOYABLES[k] for k in GROUP - {"am"})},
                   {"source": newest, "tree": tree, "deployables": sorted(DEPLOYABLES[k] for k in ("am", "eg"))}]
        receipts = {k: (fixture_runtime_receipt(component=k, source=runtime_source, tree=tree)
                        if k.startswith("runtime") else fixture_receipt(component=k, source=newest, tree=tree))
                    for k in GROUP | {"eg"}}
        self.tags = [fixture_tag(receipt) for receipt in receipts.values()]
        self.assertFalse(any(policy.transition(source) for source in (runtime_source, newest)))
        self.assertEqual(transition_in_window(self.root, history, changes), self.anchor)
        matrix = self.allocate(newest, changes)
        lanes, batches = routes(matrix, lambda: policy)
        self.assertEqual(lanes, {**dict.fromkeys(GROUP, "contract"), "eg": "eg"})
        for lane, members in (("contract", GROUP), ("eg", {"eg"})):
            admitted, failures = automatic_admission(batches[lane], self.api, self.registry, policy=policy)
            self.assertFalse(failures)
            check_lane(lane, batches[lane], admitted, failures)
            self.assertEqual(admitted["receipts"], {k: receipts[k] for k in members})
            self.assertEqual(admitted.get("contract_transition"), self.anchor if lane == "contract" else None)

    def test_deploy_checkout_can_verify_the_parent_contract_without_lazy_fetch(self):
        workflow = Path(__file__).resolve().parents[1] / "workflows/staging-deploy.yml"
        steps = yaml.safe_load(workflow.read_text())["jobs"]["deploy"]["steps"]
        verify = next(i for i, step in enumerate(steps) if step.get("env", {}).get("CD_RESOLVE_MODE") == "verify")
        checkout, = [s for s in steps[:verify] if s.get("uses", "").startswith("actions/checkout@")
                     and "repository" not in s.get("with", {})]
        options = checkout.get("with", {})
        self.assertIs(options.get("persist-credentials"), False)
        self.assertNotIn("sparse-checkout", options)
        depth = options.get("fetch-depth", 1)
        admitted, errors = automatic_admission(self.successor(), self.api, self.registry, policy=self.policy)
        self.assertFalse(errors)
        origin = self.root.as_uri()
        subprocess.run(["git", "config", "uploadpack.allowFilter", "true"], cwd=self.root, check=True)
        def fetch_local(args, **kwargs):
            public = "https://github.com/ChronoAIProject/cma.git"
            return subprocess.run([origin if arg == public else arg for arg in args], **kwargs)
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp) / "deploy"
            subprocess.run(["git", "clone", "-q", "--no-tags", *(["--depth", str(depth)] if depth else []),
                            origin, root], check=True, timeout=15)
            self.api.history.return_value = DevelopHistory.from_head(self.head, root=root, runner=fetch_local)
            with chdir(root):
                self.assertEqual(verify_set(admitted, self.api, self.registry, automatic=True), admitted)

    def test_refused_contract_read_preserves_only_independently_rechecked_receipts(self):
        cases = [
            # independent gateway, failed authority check, controller exit, expected overall outcome
            (True, None, 0, CONTRACT_READ_FAILED),
            (False, None, 0, CONTRACT_READ_FAILED),
            (True, 1, 0, NotAdmitted),
            (True, 2, 0, NotAdmitted),
            (True, None, 20, 20),
        ]
        for gateway, failed_check, controller_code, expected in cases:
            with self.subTest(gateway=gateway, failed_check=failed_check, controller=controller_code), \
                    tempfile.TemporaryDirectory() as tmp:
                members = GROUP | ({"eg"} if gateway else set())
                # A final exists only where its lane was selected. An EG final
                # at this source would make that source EG's baseline.
                self.tags = [fixture_tag(self.receipts[k]) for k in sorted(members)]
                row = dict(self.rows[0], deployables=sorted(DEPLOYABLES[k] for k in members))
                admitted, errors = automatic_admission(self.allocate(self.first, [row]), self.api,
                                                       self.registry, policy=self.policy)
                self.assertFalse(errors)
                with patch("staging_cd_set.ContractPolicy", return_value=self.policy):
                    locked = verify_set(admitted, self.api, self.registry, automatic=True)
                self.assertEqual(locked["receipts"].keys(), members)

                root = Path(tmp)
                env = invoke_fixtures.InvokeTests().environment(root)
                env["CD_RELEASE_SET"] = canonical(locked)
                summary = root / "summary"
                env["GITHUB_STEP_SUMMARY"] = str(summary)
                controller = root / "staging/scripts/deploy_set.py"
                controller.parent.mkdir(parents=True)
                controller.with_name("applied_contract.py").touch()
                checks = []
                def check(receipts):
                    checks.append(set(receipts))
                    if len(checks) == failed_check:
                        raise Refusal("Shared version inventory is unavailable")
                helper = Mock(return_value=30)
                def read(*args):
                    return configured_group(*args, runner=helper)
                def run(_script, path, *_args):
                    self.assertEqual(json.loads(path.read_text())["components"].keys(), {"eg"})
                    if controller_code == 0:
                        fixture_result(path)
                    return controller_code
                runner = Mock(side_effect=run)
                args = env, Mock(), lambda _: controller, runner
                def policy(_script, keys, **_kwargs):
                    return sorted(keys)
                with patch("staging_cd_invoke.require_current_versions", side_effect=check), \
                        patch("staging_cd_invoke.configured_group", side_effect=read):
                    if expected is NotAdmitted:
                        with self.assertRaises(NotAdmitted):
                            deploy(*args, policy_reader=policy)
                    else:
                        self.assertEqual(deploy(*args, policy_reader=policy), expected)
                self.assertEqual(checks, [members, {"eg"}] if gateway and failed_check != 1 else [members])
                self.assertEqual(helper.call_count, int(failed_check != 1))
                self.assertEqual(runner.call_count, int(gateway and failed_check is None))
                if failed_check != 1:
                    self.assertIn(", ".join(sorted(GROUP)), summary.read_text())
                if gateway and expected == CONTRACT_READ_FAILED:
                    result = json.loads((root / "public-deployment/deployment.json").read_text())
                    self.assertEqual(result["components"].keys(), {"eg"})
                    self.assertEqual(result["components"]["eg"]["state"], "controller-verified")

    def test_missing_or_incompatible_peer_cannot_authorize_partial_transition(self):
        matrix = self.successor()
        peer = next(t for t in self.tags if t["receipt"]["identity"]["component"] == "runtime")
        self.tags.remove(peer)
        selected, failures = automatic_admission(matrix, self.api, self.registry, policy=self.policy)
        self.assertIsNone(selected)
        self.assertEqual(failures.keys(), GROUP)
        bad = copy.deepcopy(peer)
        bad["receipt"]["compatibility"]["bootstrap"] = "9"
        self.tags.append(bad)
        selected, failures = automatic_admission(matrix, self.api, self.registry, policy=self.policy)
        self.assertIsNone(selected)
        self.assertEqual(failures.keys(), GROUP)
        with self.assertRaises(Refusal):
            automatic_admission({"include": matrix["include"][:1]}, self.api, self.registry, policy=self.policy)

    def test_active_unfinalized_peer_is_reallocated_from_its_burned_watermark(self):
        peer = next(t for t in self.tags if t["receipt"]["identity"]["component"] == "runtime")
        self.tags.remove(peer)
        item = dict(peer["receipt"]["identity"], schema="cma-version-reservation-v1")
        item.pop("build")
        self.store.list.return_value = [item]
        matrix = self.allocate(self.head, self.rows)
        entry = next(e for e in matrix["include"] if e["component"] == "runtime")
        task, = json.loads(entry["tasks"])
        self.assertEqual((task["mode"], task["reservation"]["previous"], task["reservation"]["source"]),
                         ("publish", item["version"], self.first))
