"""An explicit private switch is the only authority to enter a deployment."""
import tempfile
import unittest
from pathlib import Path

import yaml

from staging_cd_common import Refusal
from staging_cd_policy import enabled


class DeploymentPolicyTests(unittest.TestCase):
    def test_enabled_and_disabled_are_independent_per_component(self):
        with tempfile.TemporaryDirectory() as directory:
            script = Path(directory) / "staging/scripts/deploy.py"
            script.parent.mkdir(parents=True)
            config = script.parent.parent / "config.yaml"
            config.write_text("backend: disabled\napps:\n  am: {deploy: disabled}\n  eg: {deploy: disabled}\n  web: {deploy: enabled}\n")
            self.assertTrue(enabled(script, "web"))
            self.assertFalse(enabled(script, "am"))
            self.assertFalse(enabled(script, "eg"))

    def test_absent_malformed_duplicate_linked_or_extra_targets_fail_closed(self):
        with tempfile.TemporaryDirectory() as directory:
            script = Path(directory) / "staging/scripts/deploy.py"
            script.parent.mkdir(parents=True)
            config = script.parent.parent / "config.yaml"
            with self.assertRaises(Refusal):
                enabled(script, "web")
            valid = "apps:\n  am: {deploy: disabled}\n  eg: {deploy: disabled}\n  web: {deploy: enabled}\n"
            for data in ("[]", "apps: []", valid.replace("deploy: enabled", ""),
                         valid.replace("deploy: enabled", "deploy: true"),
                         valid.replace("deploy: enabled", "deploy: disabled, deploy: enabled"),
                         valid + "  unrelated: {deploy: enabled}\n", valid + "apps: {}\n",
                         valid.replace("deploy: enabled", "<<: {deploy: enabled}")):
                config.write_text(data)
                with self.subTest(data=data), self.assertRaises((Refusal, yaml.YAMLError)):
                    enabled(script, "web")
            config.write_text(valid)
            with self.assertRaises(Refusal):
                enabled(script, "unrelated")
            outside = Path(directory) / "outside.yaml"
            outside.write_text(valid)
            config.unlink()
            config.symlink_to(outside)
            with self.assertRaises(Refusal):
                enabled(script, "web")
