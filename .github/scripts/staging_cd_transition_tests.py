"""A later AM change owns any still-outstanding peers of a contract transition."""
import copy
import json
from pathlib import Path
import subprocess
import tempfile
import unittest
from unittest.mock import Mock, patch

from staging_cd_allocate import allocate_queue
from staging_cd_allocation_fixture import AllocationServer
from staging_cd_common import Refusal
from staging_cd_compatibility import ContractPolicy, GROUP, RECIPE, VERSION_FILE
from staging_cd_component_admission import automatic_admission
from staging_cd_contract_tests import fixture_source
from staging_cd_deploy_result_tests import fixture_result
from staging_cd_history import DevelopHistory
from staging_cd_invoke import deploy, UNVERIFIED_COHORT
import staging_cd_invoke_tests as invoke_fixtures
from staging_cd_receipts import canonical
from staging_cd_receipts_tests import fixture_receipt, fixture_runtime_receipt, fixture_tag
from staging_cd_set import verify_set
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

    def allocate(self, head, changes):
        return allocate_queue({"head": head, "event_source": head, "changes": changes,
                               "contract_transition": self.anchor}, self.api, self.store, 999, 1)

    def successor(self):
        matrix = self.allocate(self.head, self.rows)
        self.assertEqual({e["component"] for e in matrix["include"]}, GROUP)
        for entry in matrix["include"]:
            task, = json.loads(entry["tasks"])
            if entry["component"] == "am":
                self.assertEqual(task["mode"], "publish")
                identity = {k: v for k, v in task["reservation"].items() if k != "schema"}
                receipt = fixture_receipt(**identity)
                self.tags.append(fixture_tag(receipt))
            else:
                self.assertEqual(task["receipt"], self.receipts[entry["component"]])
        return matrix

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
        # Reconciliation without a new publication must not replay the transition.
        self.assertEqual(self.allocate("d" * 40, self.rows), {"include": []})

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
