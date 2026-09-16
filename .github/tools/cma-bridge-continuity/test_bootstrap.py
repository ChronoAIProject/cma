import ast
import base64
import json
import shlex
import unittest
from pathlib import Path

import yaml

from authority import guard_resource, new_plan
from bootstrap import build_resources


class BootstrapTests(unittest.TestCase):
    def test_generated_environment_has_separate_database_and_provider_authority(self):
        repo = Path(__file__).resolve().parents[3]
        plan = new_plan("fixture-context", "fixture-owner")
        config = yaml.safe_load((repo / "infra/api/configmap.yaml").read_text())
        api = yaml.safe_load((repo / "infra/api/deployment.yaml").read_text())
        production = {
            "config": config, "api": api,
            "secrets": {"data": {key: base64.b64encode(key.encode()).decode() for key in ["CHRONO_NYXID_CLIENT_SECRET", "CHRONO_STORAGE_CLIENT_SECRET", "CHRONO_MONGODB_URI", "CHRONO_AUTH_ENCRYPTION_KEY"]}},
            "provider": {"spec": {"template": {"spec": {"containers": [{"image": "registry.example/provider@sha256:" + "a" * 64}]}}}},
            "provider_role": {"rules": [{"apiGroups": [""], "resources": ["pods"], "verbs": ["get"]}]},
            "provider_template": {"data": {"batchsandbox-template.yaml": yaml.safe_dump({"metadata": {"labels": {}}, "spec": {"template": {"metadata": {"labels": {}}, "spec": {}}}})}},
        }
        resources, snapshot, api_resources = build_resources(repo, plan, production)
        for resource in resources + [snapshot] + api_resources:
            guard_resource(plan, resource)
        settings = api_resources[0]["data"]
        self.assertEqual(settings["CHRONO_MONGODB_DATABASE"], plan["namespace"])
        self.assertEqual(settings["CHRONO_DEPLOYMENT_ID"], plan["deployment_id"])
        self.assertEqual(settings["CHRONO_SANDBOX_NAMESPACE"], plan["namespace"])
        self.assertIn(plan["namespace"] + ".svc.cluster.local", settings["CHRONO_SANDBOX_BASE_URL"])
        secret = next(item for item in resources if item["kind"] == "Secret" and item["metadata"]["name"] == "cma-api")
        self.assertEqual(base64.b64decode(secret["data"]["CHRONO_MONGODB_URI"]).decode(), f"mongodb://mongo.{plan['namespace']}.svc.cluster.local:27017/?replicaSet=continuity")
        self.assertNotEqual(secret["data"]["CHRONO_AUTH_ENCRYPTION_KEY"], production["secrets"]["data"]["CHRONO_AUTH_ENCRYPTION_KEY"])
        self.assertEqual(len(base64.b64decode(base64.b64decode(secret["data"]["CHRONO_AUTH_ENCRYPTION_KEY"]))), 32)
        self.assertEqual(api_resources[1]["spec"]["template"]["spec"]["serviceAccountName"], "cma-api")
        provider = next(item for item in resources if item["kind"] == "Deployment")
        startup = provider["spec"]["template"]["spec"]["containers"][0]["command"][-1]
        python_command = shlex.split(startup.splitlines()[0])
        self.assertEqual(python_command[:2], ["python3", "-c"])
        ast.parse(python_command[2])
        provider_config = next(item for item in resources if item["kind"] == "ConfigMap" and item["metadata"]["name"] == "provider")
        self.assertNotIn("chrono-bot", json.dumps(provider_config))


if __name__ == "__main__":
    unittest.main()
