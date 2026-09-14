import unittest
from unittest.mock import patch

from authority import new_plan
from preflight import check_permissions, require_permissions


class PreflightTests(unittest.TestCase):
    def setUp(self):
        self.plan = new_plan("fixture-context", "fixture-owner")
        self.role = {"apiVersion": "rbac.authorization.k8s.io/v1", "kind": "Role", "metadata": {
            "name": "cma-api", "namespace": self.plan["namespace"],
            "labels": {"cma/continuity-run": self.plan["run"]},
        }, "rules": []}

    def test_denied_permissions_are_reported_without_writing_a_role(self):
        with patch("preflight.kubectl", return_value={"status": {"allowed": False}}) as kubectl:
            with self.assertRaisesRegex(ValueError, "fixture_create_permissions_missing:roles.rbac.authorization.k8s.io"):
                require_permissions(self.plan, [self.role])
            submitted = kubectl.call_args.args[2]
            self.assertEqual(submitted["kind"], "SelfSubjectAccessReview")
            self.assertEqual(submitted["spec"]["resourceAttributes"]["namespace"], self.plan["namespace"])

    def test_same_kind_is_checked_once_and_missing_result_denies(self):
        with patch("preflight.kubectl", return_value={}) as kubectl:
            self.assertEqual(len(check_permissions(self.plan, [self.role, self.role])), 1)
            kubectl.assert_called_once()

    def test_granted_access_allows_bootstrap_to_continue(self):
        with patch("preflight.kubectl", return_value={"status": {"allowed": True}}):
            require_permissions(self.plan, [self.role])


if __name__ == "__main__":
    unittest.main()
