import copy
import unittest

from authority import guard_mutation, guard_resource, new_plan, record_created


class AuthorityTests(unittest.TestCase):
    def setUp(self):
        self.plan = new_plan("fixture-context", "fixture-owner")
        self.namespace = {"apiVersion": "v1", "kind": "Namespace", "metadata": {
            "name": self.plan["namespace"], "uid": "namespace-original", "resourceVersion": "1",
            "labels": {"cma/continuity-run": self.plan["run"]},
        }}
        record_created(self.plan, self.namespace)
        self.resource = {"apiVersion": "v1", "kind": "ConfigMap", "metadata": {
            "name": "fixture-config", "namespace": self.plan["namespace"], "uid": "config-original", "resourceVersion": "2",
            "labels": {"cma/continuity-run": self.plan["run"]},
        }}

    def test_mutation_requires_created_receipt_and_current_uid(self):
        with self.assertRaisesRegex(ValueError, "unjournaled"):
            guard_mutation(self.plan, self.namespace, self.resource)
        record_created(self.plan, self.resource)
        self.assertEqual(guard_mutation(self.plan, self.namespace, self.resource), {"uid": "config-original", "resourceVersion": "2"})
        self.resource["metadata"]["uid"] = "replacement"
        with self.assertRaisesRegex(ValueError, "replaced"):
            guard_mutation(self.plan, self.namespace, self.resource)

    def test_production_and_foreign_run_resources_are_refused(self):
        for modify in [
            lambda r: r["metadata"].update(namespace="chrono-bot"),
            lambda r: r["metadata"]["labels"].update({"cma/continuity-run": "another-run"}),
            lambda r: r["metadata"].update(labels={}),
        ]:
            resource = copy.deepcopy(self.resource)
            modify(resource)
            with self.assertRaises(ValueError):
                guard_resource(self.plan, resource)

    def test_namespace_recreation_revokes_every_old_receipt(self):
        record_created(self.plan, self.resource)
        self.namespace["metadata"]["uid"] = "replacement-namespace"
        with self.assertRaisesRegex(ValueError, "namespace_replaced"):
            guard_mutation(self.plan, self.namespace, self.resource)

    def test_existing_global_roles_and_cross_namespace_subjects_are_refused(self):
        resource = {"apiVersion": "rbac.authorization.k8s.io/v1", "kind": "ClusterRoleBinding", "metadata": {
            "name": self.plan["namespace"], "labels": {"cma/continuity-run": self.plan["run"]},
        }, "subjects": [{"kind": "ServiceAccount", "name": "default", "namespace": "chrono-bot"}], "roleRef": {"name": "cluster-admin"}}
        with self.assertRaisesRegex(ValueError, "foreign_role_subject"):
            guard_resource(self.plan, resource)
        resource["subjects"][0].update(namespace=self.plan["namespace"], name="provider")
        with self.assertRaisesRegex(ValueError, "foreign_cluster_role"):
            guard_resource(self.plan, resource)

    def test_cluster_authority_is_only_reading_the_exact_runtimeclass(self):
        resource = {"apiVersion": "rbac.authorization.k8s.io/v1", "kind": "ClusterRole", "metadata": {
            "name": self.plan["namespace"], "labels": {"cma/continuity-run": self.plan["run"]},
        }, "rules": [{"apiGroups": ["node.k8s.io"], "resources": ["runtimeclasses"], "resourceNames": ["gvisor"], "verbs": ["get"]}]}
        guard_resource(self.plan, resource)
        resource["rules"][0]["verbs"].append("delete")
        with self.assertRaisesRegex(ValueError, "excessive_cluster_authority"):
            guard_resource(self.plan, resource)

    def test_labeled_user_namespace_does_not_become_a_fixture(self):
        self.plan["namespace"] = "chrono-bot"
        with self.assertRaisesRegex(ValueError, "invalid_fixture_scope"):
            guard_resource(self.plan, self.resource)


if __name__ == "__main__":
    unittest.main()
