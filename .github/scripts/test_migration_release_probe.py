import json
import unittest
from unittest.mock import Mock

from migration_image_preflight import RunnerError
from migration_release_probe import (
    POLICY_ENV, declared_artifacts, policy_env, stage_schedule, verify_pod_policy,
)


def artifact(index):
    return {"image": "ghcr.io/example-owner/cma-api@sha256:" + str(index) * 64,
            "source": str(index) * 40}


class ReleaseProbeTests(unittest.TestCase):
    def setUp(self):
        self.artifacts = dict(zip(("historical", "current", "candidate", "recovery"),
                                  (artifact(i) for i in range(1, 5))))
        self.plan = {"readers": [{"id": role, "role": role, "artifact": value}
                                 for role, value in self.artifacts.items()]}
        self.plan["candidate"] = self.artifacts["candidate"]
        self.inspect = Mock(side_effect=lambda **value: {**value, "config": "fixture"})

    def test_distinct_roles_and_both_writer_policies(self):
        for policy in ("disabled", "enabled"):
            actual = declared_artifacts(self.plan, self.artifacts["historical"],
                                        self.artifacts["candidate"], policy, self.inspect)
            self.assertEqual({role: row["source"] for role, row in actual.items()},
                             {role: row["source"] for role, row in self.artifacts.items()})
            stages = stage_schedule({role: role for role in actual}, actual, policy)
            self.assertEqual([row[2] for row in stages], list(self.artifacts))
            self.assertEqual([row[4] for row in stages], ["disabled", "disabled", policy, "disabled"])
            self.assertTrue(all(row[1] for row in stages))
            config = json.loads(policy_env(policy)["value"])
            self.assertEqual(config["mode"], "all" if policy == "enabled" else "disabled")
            self.assertEqual(len(config["capabilities"]), 2 if policy == "enabled" else 0)

    def test_distinct_roles_require_explicit_policy_and_matching_inputs(self):
        for policy, old, new in ((None, self.artifacts["historical"], self.artifacts["candidate"]),
                                 ("enabled", artifact(5), self.artifacts["candidate"]),
                                 ("enabled", self.artifacts["historical"], artifact(5))):
            with self.assertRaises(RunnerError):
                declared_artifacts(self.plan, old, new, policy, self.inspect)
        self.plan["candidate"] = artifact(5)
        with self.assertRaises(RunnerError):
            declared_artifacts(self.plan, self.artifacts["historical"],
                               self.artifacts["candidate"], "enabled", self.inspect)

    def test_policy_readback_is_bound_to_selected_pod(self):
        selected = {"uid": "fixture-current"}
        pods = {"items": [{"metadata": selected, "spec": {"containers": [
            {"name": "api", "env": [policy_env("enabled")]}
        ]}}]}
        verify_pod_policy(pods, selected, "enabled")
        with self.assertRaises(RunnerError):
            verify_pod_policy(pods, selected, "disabled")
        with self.assertRaises(RunnerError):
            verify_pod_policy(pods, {"uid": "fixture-other"}, "enabled")
        pods["items"][0]["spec"]["containers"][0]["env"].append(
            {"name": POLICY_ENV, "value": "duplicate"})
        with self.assertRaises(RunnerError):
            verify_pod_policy(pods, selected, "enabled")
