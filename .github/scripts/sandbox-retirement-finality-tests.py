#!/usr/bin/env python3
"""Verify exact provider identity and retained volume disposition."""

import unittest

from sandbox_retirement_finality import Finality, evaluate


class FinalityTests(unittest.TestCase):
    def setUp(self):
        self.expected = {
            name: {"name": name, "uid": f"original-{name}"}
            for name in ("sandbox", "claim", "pvc", "pv")
        }
        self.original_pod = {"name": "pod", "uid": "original-pod"}
        self.expected.update(namespace="fixture", storage="retain_bound", pods=[self.original_pod])
        self.observed = {"sandbox": None, "pods": [], "claim": None}
        self.observed["pvc"] = {
            "metadata": {**self.expected["pvc"], "namespace": "fixture"},
            "spec": {"volumeName": "pv"},
            "status": {"phase": "Bound"},
        }
        self.observed["pv"] = {
            "metadata": self.expected["pv"].copy(),
            "spec": {
                "persistentVolumeReclaimPolicy": "Retain",
                "claimRef": {**self.expected["pvc"], "namespace": "fixture"},
            },
            "status": {"phase": "Bound"},
        }

    def test_original_pvc_and_released_pv_are_separate_expected_dispositions(self):
        self.assertIs(evaluate(self.expected, self.observed), Finality.SETTLED)
        self.expected["storage"] = "retain_released"
        self.assertIs(evaluate(self.expected, self.observed), Finality.PENDING)
        self.observed["pvc"] = None
        self.observed["pv"]["status"]["phase"] = "Released"
        self.assertIs(evaluate(self.expected, self.observed), Finality.SETTLED)
        self.observed["pv"]["spec"]["claimRef"]["uid"] = "different-pvc-uid"
        self.assertIs(evaluate(self.expected, self.observed), Finality.INVALID)

    def test_same_name_replacements_cannot_complete_an_original_retirement(self):
        for kind in ("sandbox", "claim", "pvc", "pv"):
            with self.subTest(kind=kind):
                replaced = {**self.observed, kind: {"metadata": {"name": kind, "uid": "replacement"}}}
                self.assertIs(evaluate(self.expected, replaced), Finality.REPLACED)
        self.observed["pods"] = [{"metadata": {"name": "pod", "uid": "replacement", "namespace": "fixture"}}]
        self.assertIs(evaluate(self.expected, self.observed), Finality.REPLACED)

    def test_a_terminating_original_or_missing_provider_read_remains_unresolved(self):
        self.observed["pods"] = [{
            "metadata": {**self.original_pod, "namespace": "fixture", "deletionTimestamp": "2026-01-02T00:00:00Z"}
        }]
        self.assertIs(evaluate(self.expected, self.observed), Finality.PENDING)
        del self.observed["pods"]
        self.assertIs(evaluate(self.expected, self.observed), Finality.INVALID)

    def test_an_unlisted_pod_of_the_original_sandbox_blocks_completion(self):
        self.observed["pods"] = [{"metadata": {
            "name": "unlisted-pod",
            "uid": "another-uid",
            "namespace": "fixture",
            "ownerReferences": [{
                "apiVersion": "agents.x-k8s.io/v1beta1", "kind": "Sandbox",
                "uid": self.expected["sandbox"]["uid"],
            }],
        }}]
        self.assertIs(evaluate(self.expected, self.observed), Finality.PENDING)
        self.observed["pods"][0]["metadata"]["ownerReferences"][0]["uid"] = "unrelated"
        self.assertIs(evaluate(self.expected, self.observed), Finality.SETTLED)


if __name__ == "__main__":
    unittest.main()
