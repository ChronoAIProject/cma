"""A coordinated admission binds every selected version, source and digest."""
import copy
import unittest
from unittest.mock import Mock

from staging_cd_common import Refusal
from staging_cd_history import DevelopHistory
from staging_cd_receipts import canonical
from staging_cd_receipts_tests import fixture_receipt, fixture_tag, fixture_runtime_receipt
from staging_cd_set import (automatic_set, dispatch_set, document, private_document,
                            release_set, selection, validate_set, verify_set)


class SetTests(unittest.TestCase):
    def setUp(self):
        self.receipts = {key: fixture_receipt(component=key) for key in ("am", "eg", "web")}
        self.receipts["eg"] = fixture_receipt(component="eg", previous="0.11.0", version="0.11.1")
        self.api = Mock()
        self.api.history.return_value = DevelopHistory("a" * 40, ["a" * 40])
        self.api.tags.side_effect = lambda key: [fixture_tag(self.receipts[key])]
        self.registry = Mock()
        self.registry.exists.return_value = True
        self.registry.manifest.return_value = ("sha256:" + "d" * 64, {})

    def test_independent_versions_freeze_exact_complete_set(self):
        inputs = {"deployables": "web,am,eg", "mode": "bootstrap",
                  "versions": '{"am":"0.10.1","eg":"0.11.1","web":"0.10.1"}'}
        mode, value = dispatch_set(inputs, self.api, self.registry)
        self.assertEqual(mode, "bootstrap")
        self.assertEqual(value, release_set(self.receipts))
        self.assertEqual(verify_set(value, self.api, self.registry, automatic=False, inputs=inputs), value)
        self.assertEqual(private_document(value)["components"]["eg"],
                         {"image": "ghcr.io/chronoaiproject/cma-event-gateway@sha256:" + "d" * 64,
                          "source_sha": "a" * 40, "version": "0.11.1"})
        self.api.request.assert_not_called()

    def test_no_ambiguous_versions_duplicates_or_partial_bootstrap(self):
        for inputs in ({"deployables": "web", "mode": "bootstrap"},
                       {"deployables": "am,eg", "version": "0.10.1"},
                       {"deployables": "web", "versions": '{"am":"0.10.1"}'},
                       {"deployables": "web", "versions": '{"web":"0.10.1","web":"0.10.2"}'},
                       {"deployables": "web", "versions": '{"web":"latest"}'},
                       {"deployables": "web", "versions": '{"web":"0.10.1"}', "version": "0.10.1"},
                       {"deployables": "web", "mode": "other"},
                       {"deployables": "am,am"}, {"deployables": "other"}):
            with self.subTest(inputs=inputs), self.assertRaises((Refusal, ValueError)):
                dispatch_set(inputs, self.api, self.registry)
        self.api.tags.assert_not_called()
        self.assertEqual(selection({"deployables": "web", "version": "0.10.1"}),
                         ("release", {"web": "0.10.1"}))
        self.assertEqual(selection({"deployables": "web"}), ("release", {"web": "latest"}))

    def matrix(self, keys, *, resume=False):
        entries = []
        for component in keys:
            receipt = self.receipts[component]
            if resume:
                task = {"mode": "resume", "component": component, "receipt": receipt}
            else:
                reservation = dict(receipt["identity"], schema="cma-version-reservation-v1")
                reservation.pop("build")
                task = {"mode": "publish", "component": component, "reservation": reservation}
            entries.append({"component": component, "tasks": canonical([task])})
        return {"include": entries}

    def test_automatic_requires_every_expected_publication_and_resumes_without_writes(self):
        for resume in (False, True):
            matrix = self.matrix(["am", "eg", "web"], resume=resume)
            self.assertEqual(automatic_set(matrix, self.api, self.registry), release_set(self.receipts))
            self.api.tags.side_effect = lambda key: [] if key == "eg" else [fixture_tag(self.receipts[key])]
            with self.assertRaises(Refusal):
                automatic_set(matrix, self.api, self.registry)
            self.api.tags.side_effect = lambda key: [fixture_tag(self.receipts[key])]
        self.api.request.assert_not_called()

    def test_missing_tail_publication_cannot_deploy_successful_prefix(self):
        matrix = self.matrix(["web"])
        tasks = document(matrix["include"][0]["tasks"])
        task = copy.deepcopy(tasks[0])
        task["reservation"].update(source="f" * 40, previous="0.10.1", version="0.10.2")
        matrix["include"][0]["tasks"] = canonical([*tasks, task])
        with self.assertRaises(Refusal):
            automatic_set(matrix, self.api, self.registry)

    def test_empty_publication_has_no_deployment(self):
        self.assertIsNone(automatic_set({"include": []}, self.api, self.registry))
        self.api.tags.assert_not_called()

    def test_runtime_receipts_are_admitted_alone_or_with_apps(self):
        for key in ("runtime", "runtime-omarchy"):
            self.receipts[key] = fixture_runtime_receipt(component=key)
        for keys in (("runtime",), ("runtime-omarchy",), ("am", "runtime", "runtime-omarchy")):
            for resume in (False, True):
                expected = release_set({key: self.receipts[key] for key in keys})
                self.assertEqual(automatic_set(self.matrix(keys, resume=resume), self.api, self.registry), expected)
                private = private_document(expected)
                self.assertEqual(set(private["components"]), set(keys))
                self.assertEqual(verify_set(expected, self.api, self.registry, automatic=True), expected)
        self.api.request.assert_not_called()

    def test_runtime_policy_a_failure_refuses_whole_set(self):
        self.receipts["runtime"] = fixture_runtime_receipt()
        self.receipts["runtime"]["scan"]["complete"] = False
        with self.assertRaises(Refusal):
            automatic_set(self.matrix(("web", "runtime")), self.api, self.registry)

    def test_runtime_dispatch_does_not_expand_bootstrap_authority(self):
        for key in ("runtime", "runtime-omarchy"):
            self.receipts[key] = fixture_runtime_receipt(component=key)
        mode, value = dispatch_set({"deployables": "runtime,runtime-omarchy"}, self.api, self.registry)
        self.assertEqual(mode, "release")
        self.assertEqual(set(value["receipts"]), {"runtime", "runtime-omarchy"})
        for selected in ("runtime", "am,eg,web,runtime,runtime-omarchy"):
            with self.assertRaises(Refusal):
                dispatch_set({"deployables": selected, "mode": "bootstrap"}, self.api, self.registry)

    def test_set_shape_scan_binding_and_dispatch_members_cannot_change(self):
        value = release_set(self.receipts)
        for bad in ({}, dict(value, private="forbidden"), dict(value, receipts={}),
                    dict(value, receipts={"other": self.receipts["web"]}),
                    dict(value, receipts={"web": self.receipts["eg"]})):
            with self.assertRaises(Refusal):
                validate_set(bad)
        incomplete = copy.deepcopy(value)
        incomplete["receipts"]["web"]["scan"]["complete"] = False
        with self.assertRaises(Refusal):
            validate_set(incomplete)
        for inputs in ({"deployables": "web"}, {"deployables": "am,eg,web", "versions":
                       '{"am":"0.10.2","eg":"0.11.1","web":"0.10.1"}'}):
            with self.assertRaises(Refusal):
                verify_set(value, self.api, self.registry, automatic=False, inputs=inputs)
        with self.assertRaises(Refusal):
            document('{"schema":1,"schema":2}')

    def test_mismatched_reservation_and_duplicate_groups_refuse(self):
        matrix = self.matrix(["web"])
        tasks = document(matrix["include"][0]["tasks"])
        tasks[0]["reservation"]["source"] = "b" * 40
        matrix["include"][0]["tasks"] = canonical(tasks)
        with self.assertRaises(Refusal):
            automatic_set(matrix, self.api, self.registry)
        matrix = self.matrix(["web", "web"])
        with self.assertRaises(Refusal):
            automatic_set(matrix, self.api, self.registry)


if __name__ == "__main__":
    unittest.main()
