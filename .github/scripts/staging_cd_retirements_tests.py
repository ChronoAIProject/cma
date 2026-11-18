"""Retirement preserves burned history and requires genuine selected successors."""
import copy
import hashlib
import json
import unittest
from unittest.mock import Mock, patch

from staging_cd_accounting import account
from staging_cd_allocate import allocate_queue
from staging_cd_common import Refusal
from staging_cd_history import DevelopHistory
from staging_cd_receipts_tests import fixture_receipt, fixture_tag
from staging_cd_retirements import (
    BURN_OBJECTS, INPUT_ROOT, SOURCE, TREE, original_reservation, parse_policy,
    preserve_decisions, resolve_retirements,
)

REPLACEMENT, HEAD, NEW_TREE = (letter * 40 for letter in "def")
PACKAGING = b"synthetic packaging input\n"


def fixture_policy():
    return {"schema": "cma-source-retirements-v1", "decisions": [
        {"id": "retire-" + component + "-730c1bfc", "reason": "private-key-test-material",
         "reservation": original_reservation(component), "reservation_tag": "cma-" + component + "/pending/v0.10.1",
         "reservation_object": BURN_OBJECTS[component],
         "replacement_pr": 4242, "packaging_inputs": {INPUT_ROOT + "Dockerfile": hashlib.sha256(PACKAGING).hexdigest()}}
        for component in ("runtime", "runtime-desktop")]}


class RetirementTests(unittest.TestCase):
    def setUp(self):
        self.policy = parse_policy(json.dumps(fixture_policy()).encode())
        self.queue = {"head": HEAD, "event_source": HEAD, "changes": [
            {"source": SOURCE, "tree": TREE, "deployables": ["cma-runtime", "cma-runtime-desktop", "cma-web"]},
            {"source": REPLACEMENT, "tree": NEW_TREE, "deployables": ["cma-runtime", "cma-runtime-desktop"]}]}
        self.api = Mock()
        self.api.history.return_value = DevelopHistory(HEAD, [HEAD, REPLACEMENT, SOURCE])
        self.pr = {"number": 4242, "merged": True, "merged_at": "synthetic", "merge_commit_sha": REPLACEMENT,
                   "base": {"ref": "develop", "repo": {"full_name": "ChronoAIProject/cma"}},
                   "labels": [{"name": "version-upgrade:patch"}]}
        self.api.request.side_effect = self.request
        self.pending = [original_reservation(key) for key in ("runtime", "runtime-desktop")]

    def request(self, path):
        if path.startswith("git/ref/tags/"):
            component = "runtime-desktop" if "runtime-desktop" in path else "runtime"
            return {"ref": "refs/tags/cma-" + component + "/pending/v0.10.1",
                    "object": {"type": "tag", "sha": BURN_OBJECTS[component]}}
        if path.startswith("actions/runs/"):
            parts = path.split("/")
            return {"id": int(parts[2]), "run_attempt": int(parts[4]) if len(parts) > 4 else 2, "event": "push", "head_branch": "develop",
                    "repository": {"full_name": "ChronoAIProject/cma"}, "status": "completed"}
        if path.startswith("git/commits/"):
            return {"sha": REPLACEMENT, "tree": {"sha": NEW_TREE}}
        if path.startswith("commits/" + SOURCE) or path == "pulls/2331":
            old = dict(self.pr, number=2331, merge_commit_sha=SOURCE)
            return [old] if path.startswith("commits/") else old
        return [self.pr] if path.startswith("commits/") else self.pr

    def resolved(self, **kwargs):
        return resolve_retirements(self.policy, self.queue, self.api, None,
                                   selector=kwargs.get("selector", lambda *_: ["cma-runtime", "cma-runtime-desktop"]),
                                   read_blob=kwargs.get("read_blob", lambda *_: PACKAGING))

    def finals(self):
        return [fixture_tag(fixture_receipt(component="web", source=SOURCE, tree=TREE, pr=2331)),
                *[fixture_tag(fixture_receipt(component=component, source=REPLACEMENT, tree=NEW_TREE,
                                             pr=4242, previous="0.10.1", version="0.10.2"))
                  for component in ("runtime", "runtime-desktop")]]

    def test_exact_pairs_and_unchanged_burns_are_required(self):
        retirement = self.resolved()
        for component in ("runtime", "runtime-desktop"):
            with self.assertRaises(Refusal):
                retirement.guard(component, SOURCE)
            retirement.guard(component, REPLACEMENT)
        retirement.guard("runtime-omarchy", SOURCE)
        retirement.inventory([], self.pending, lambda _: False)
        for pending, active, finals in (([], False, []), (self.pending, True, []),
                                       (self.pending, False, [fixture_tag(fixture_receipt(component="runtime", source=SOURCE))])):
            with self.assertRaises(Refusal):
                retirement.inventory(finals, pending, lambda _: active)

    def test_unknown_omitted_duplicate_changed_and_wildcard_decisions_refuse(self):
        variants = []
        for mutate in (lambda d: d["decisions"].pop(), lambda d: d["decisions"].append(d["decisions"][0]),
                       lambda d: d["decisions"][0]["reservation"].update(component="runtime-omarchy"),
                       lambda d: d["decisions"][0]["reservation"].update(source="*"),
                       lambda d: d["decisions"][0].update(replacement_pr=0),
                       lambda d: d["decisions"][0].update(packaging_inputs={}),
                       lambda d: d["decisions"][0].update(extra=True)):
            value = copy.deepcopy(self.policy)
            mutate(value)
            variants.append(value)
        for value in variants:
            with self.assertRaises((Refusal, ValueError)):
                parse_policy(json.dumps(value).encode())
        changed = copy.deepcopy(self.policy)
        changed["decisions"][0]["replacement_pr"] += 1
        with self.assertRaises(Refusal):
            preserve_decisions(self.policy, changed)

    def test_unmerged_wrong_base_or_displaced_replacement_refuses(self):
        for update in ({"merged": False}, {"merge_commit_sha": SOURCE},
                       {"merge_commit_sha": "a" * 40}, {"base": {"ref": "main"}}):
            original = copy.deepcopy(self.pr)
            self.pr.update(update)
            with self.assertRaises(Refusal):
                self.resolved()
            self.pr = original
        for kwargs in ({"selector": lambda *_: []}, {"read_blob": lambda *_: b"changed"}):
            with self.assertRaises(Refusal):
                self.resolved(**kwargs)

    def test_replacement_and_retired_history_cannot_be_omitted_or_forged(self):
        original = copy.deepcopy(self.queue)
        for index in (0, 1):
            self.queue = copy.deepcopy(original)
            self.queue["changes"].pop(index)
            with self.assertRaises(Refusal):
                self.resolved()
        self.queue = copy.deepcopy(original)
        self.queue["changes"][1]["tree"] = "a" * 40
        with self.assertRaises(Refusal):
            self.resolved()

    def test_allocation_preserves_burns_and_allocates_only_real_successors(self):
        retirement = self.resolved()
        self.queue["dispositions"] = retirement.dispositions
        reservations = Mock()
        reservations.list.side_effect = lambda: list(self.pending)
        reservations.insert.side_effect = lambda value: self.pending.append(value) is None
        self.api.tags.return_value = []
        matrix = allocate_queue(self.queue, self.api, reservations, 100, 1, retirements=retirement)
        tasks = [task for chain in matrix["include"] for task in json.loads(chain["tasks"])]
        self.assertEqual(len(tasks), 3)
        for task in tasks:
            item = task["reservation"]
            if task["component"].startswith("runtime"):
                self.assertEqual((item["source"], item["pr"], item["version"]), (REPLACEMENT, 4242, "0.10.2"))
        self.assertEqual(self.pending[:2], [original_reservation(key) for key in ("runtime", "runtime-desktop")])

    def test_forged_queue_disposition_fails_before_any_reservation_write(self):
        retirement = self.resolved()
        reservations = Mock()
        reservations.list.return_value = self.pending
        self.api.tags.return_value = []
        with self.assertRaises(Refusal):
            allocate_queue(self.queue, self.api, reservations, 100, 1, retirements=retirement)
        reservations.insert.assert_not_called()

    def test_complete_accounting_includes_companions_and_requires_replacements(self):
        retirement = self.resolved()
        self.queue["dispositions"] = retirement.dispositions
        finals = self.finals()
        self.assertEqual(account(self.queue, retirement, finals, self.pending, lambda _: False, lambda *_: True),
                         {"selected": 5, "published": 3, "retired": 2})
        for changed in (finals[:1], finals[:2], finals + [finals[0]]):
            with self.assertRaises(Refusal):
                account(self.queue, retirement, changed, self.pending, lambda _: False, lambda *_: True)
        with self.assertRaises(Refusal):
            account(self.queue, retirement, finals, self.pending, lambda _: False, lambda *_: False)


    def test_new_attempt_of_original_run_must_also_be_drained(self):
        def request(path):
            value = self.request(path)
            if path == "actions/runs/44199012676":
                value.update(run_attempt=3, status="in_progress")
            return value
        self.api.request.side_effect = request
        with self.assertRaises(Refusal):
            self.resolved()

    def test_patch_minor_major_labels_and_failed_replacement_versions(self):
        for level, expected in (("patch", "0.10.2"), ("minor", "0.11.0"), ("major", "1.0.0")):
            with self.subTest(level=level):
                self.pr["labels"] = [{"name": "version-upgrade:" + level}]
                self.pending = [original_reservation(key) for key in ("runtime", "runtime-desktop")]
                retirement = self.resolved()
                self.queue["dispositions"] = retirement.dispositions
                storage = Mock()
                storage.list.side_effect = lambda: list(self.pending)
                storage.insert.side_effect = lambda item: self.pending.append(item) is None
                self.api.tags.return_value = []
                matrix = allocate_queue(self.queue, self.api, storage, 100, 1, retirements=retirement)
                tasks = [task for chain in matrix["include"] for task in json.loads(chain["tasks"])]
                self.assertEqual({task["reservation"]["version"] for task in tasks
                                  if task["component"].startswith("runtime")}, {expected})
                if level == "patch":
                    retry = allocate_queue(self.queue, self.api, storage, 100, 2, retirements=retirement)
                    tasks = [task for chain in retry["include"] for task in json.loads(chain["tasks"])]
                    self.assertEqual({task["reservation"]["version"] for task in tasks
                                      if task["component"].startswith("runtime")}, {"0.10.3"})
                    self.assertEqual(sum(item["source"] == SOURCE and item["component"].startswith("runtime")
                                         for item in self.pending), 2)

    def test_collision_refresh_refuses_late_retired_allocation(self):
        retirement = self.resolved()
        self.queue["dispositions"] = retirement.dispositions
        self.api.tags.return_value = []
        storage = Mock()
        storage.list.side_effect = lambda: list(self.pending)
        def collision(_item):
            self.pending.append(dict(original_reservation("runtime"), previous="0.10.1", version="0.10.2"))
            return False
        storage.insert.side_effect = collision
        with self.assertRaises(Refusal):
            allocate_queue(self.queue, self.api, storage, 100, 1, retirements=retirement)
        storage.insert.assert_called_once()

    def test_finalized_replacement_resumes_only_its_event_and_later_push_is_empty(self):
        retirement = self.resolved()
        self.queue["dispositions"] = retirement.dispositions
        self.api.tags.return_value = self.finals()
        storage = Mock()
        storage.list.return_value = self.pending
        self.assertEqual(allocate_queue(self.queue, self.api, storage, 100, 1, retirements=retirement), {"include": []})
        self.queue["event_source"] = REPLACEMENT
        matrix = allocate_queue(self.queue, self.api, storage, 100, 2, retirements=retirement)
        tasks = [task for chain in matrix["include"] for task in json.loads(chain["tasks"])]
        self.assertEqual({task["component"] for task in tasks}, {"runtime", "runtime-desktop"})
        self.assertTrue(all(task["mode"] == "resume" for task in tasks))
        storage.insert.assert_not_called()

    def test_retired_publisher_and_finalizer_refuse_before_side_effects(self):
        from staging_cd_publish import publish
        from staging_cd_finalize import finalize
        receipt = fixture_receipt(component="runtime", source=SOURCE, tree=TREE, pr=2331)
        registry, publisher, scanner, runner = Mock(), Mock(), Mock(), Mock()
        with patch("staging_cd_retirements.load_policy", return_value=self.policy):
            with self.assertRaises(Refusal):
                publish(None, receipt["identity"], None, registry, None, None, scanner=scanner, runner=runner)
            with self.assertRaises(Refusal):
                finalize([{"component": "runtime", "mode": "resume", "receipt": receipt}],
                         [receipt], publisher, registry)
        self.assertFalse(registry.mock_calls)
        self.assertFalse(publisher.mock_calls)
        scanner.assert_not_called()
        runner.assert_not_called()

if __name__ == "__main__":
    unittest.main()
