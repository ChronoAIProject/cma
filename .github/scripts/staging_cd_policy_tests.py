"""An explicit private switch is the only authority to enter a deployment."""
import tempfile
import unittest
from pathlib import Path

import yaml

from staging_cd_common import Refusal
from staging_cd_policy import selected, switches


class DeploymentPolicyTests(unittest.TestCase):
    def test_dispatch_never_filters_a_disabled_member_into_partial_success(self):
        with tempfile.TemporaryDirectory() as directory:
            script = Path(directory) / "staging/scripts/deploy_set.py"
            script.parent.mkdir(parents=True)
            config = script.parent.parent / "config.yaml"
            config.write_text("apps:\n  am: {deploy: disabled}\n  eg: {deploy: disabled}\n  web: {deploy: enabled}\n")
            self.assertEqual(selected(script, ["am", "eg", "web"], automatic=True, mode="release"), ["web"])
            self.assertEqual(selected(script, ["am"], automatic=True, mode="release"), [])
            for mode in ("release", "bootstrap"):
                with self.assertRaises(Refusal):
                    selected(script, ["am", "eg", "web"], automatic=False, mode=mode)
            with self.assertRaises(Refusal):
                selected(script, ["am", "eg", "web"], automatic=True, mode="bootstrap")
            config.write_text(config.read_text().replace("disabled", "enabled"))
            self.assertEqual(selected(script, ["am", "eg", "web"], automatic=False, mode="bootstrap"),
                             ["am", "eg", "web"])

    def test_runtime_switch_follows_am_without_creating_an_app(self):
        with tempfile.TemporaryDirectory() as directory:
            script = Path(directory) / "staging/scripts/deploy_set.py"
            script.parent.mkdir(parents=True)
            config = script.parent.parent / "config.yaml"
            for state in ("enabled", "disabled"):
                config.write_text(f"apps:\n  am: {{deploy: {state}}}\n  eg: {{deploy: enabled}}\n  web: {{deploy: enabled}}\n")
                keys = ["runtime", "runtime-omarchy"]
                self.assertEqual(selected(script, keys, automatic=True, mode="release"),
                                 keys if state == "enabled" else [])
                if state == "disabled":
                    with self.assertRaises(Refusal):
                        selected(script, keys, automatic=False, mode="release")
                with self.assertRaises(Refusal):
                    selected(script, ["am", "eg", "web", *keys], automatic=False, mode="bootstrap")

    def test_enabled_and_disabled_are_independent_per_component(self):
        with tempfile.TemporaryDirectory() as directory:
            script = Path(directory) / "staging/scripts/deploy.py"
            script.parent.mkdir(parents=True)
            config = script.parent.parent / "config.yaml"
            config.write_text("backend: disabled\napps:\n  am: {deploy: disabled}\n  eg: {deploy: disabled}\n  web: {deploy: enabled}\n")
            self.assertTrue(switches(script)["web"])
            self.assertFalse(switches(script)["am"])
            self.assertFalse(switches(script)["eg"])

    def test_absent_malformed_duplicate_linked_or_extra_targets_fail_closed(self):
        with tempfile.TemporaryDirectory() as directory:
            script = Path(directory) / "staging/scripts/deploy.py"
            script.parent.mkdir(parents=True)
            config = script.parent.parent / "config.yaml"
            with self.assertRaises(Refusal):
                switches(script)["web"]
            valid = "apps:\n  am: {deploy: disabled}\n  eg: {deploy: disabled}\n  web: {deploy: enabled}\n"
            for data in ("[]", "apps: []", valid.replace("deploy: enabled", ""),
                         valid.replace("deploy: enabled", "deploy: true"),
                         valid.replace("deploy: enabled", "deploy: disabled, deploy: enabled"),
                         valid + "  unrelated: {deploy: enabled}\n", valid + "apps: {}\n",
                         valid.replace("deploy: enabled", "<<: {deploy: enabled}")):
                config.write_text(data)
                with self.subTest(data=data), self.assertRaises((Refusal, yaml.YAMLError)):
                    switches(script)["web"]
            config.write_text(valid)
            with self.assertRaises(Refusal):
                selected(script, ["unrelated"], automatic=False, mode="release")
            outside = Path(directory) / "outside.yaml"
            outside.write_text(valid)
            config.unlink()
            config.symlink_to(outside)
            with self.assertRaises(Refusal):
                switches(script)["web"]
