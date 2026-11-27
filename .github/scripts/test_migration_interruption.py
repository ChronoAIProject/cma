"""Cancellation must preserve partial evidence without certifying compatibility."""
from contextlib import ExitStack
import json
from pathlib import Path
import tempfile
from types import SimpleNamespace
import unittest
from unittest.mock import Mock, patch

import migration_kind_runner as runner
from migration_redaction import EvidenceRedactor


class MigrationInterruptionTests(unittest.TestCase):
    def setUp(self):
        directory = tempfile.TemporaryDirectory()
        self.addCleanup(directory.cleanup)
        root = Path(directory.name)
        plan = root / 'plan.json'
        plan.write_text(json.dumps({'schema': 'cma-migration-matrix-plan-v1',
                                    'fixtures': ['fixture'], 'cells': ['cell']}))
        self.args = SimpleNamespace(output=root / 'run', plan=plan, kind='unused',
                                    timeout=1, old_image='old', new_image='new',
                                    old_source='a' * 40, new_source='b' * 40,
                                    mongo_image='mongo', identity_image='identity',
                                    runtime_image='runtime', fixture_command='unused',
                                    inventory=root / 'inventory.json')
        self.campaign = Mock(spec=runner.Campaign)
        self.campaign.cluster = 'cma-matrix-0123456789'
        self.campaign.plan_sha256 = 'c' * 64
        self.campaign.commands = []
        self.campaign.diagnostic_errors = []
        self.campaign.redactor = EvidenceRedactor()
        self.campaign.active_reader = {'pod': {'uid': 'reader-uid'}}
        self.campaign.alias_image.side_effect = lambda image, role: role
        self.campaign.boot.side_effect = lambda image, source, stage, config: {'stage': stage}
        self.campaign.fixture.side_effect = lambda command, stage, *args: {'stage': stage}
        stack = ExitStack()
        self.addCleanup(stack.close)
        mocks = [
            patch.object(runner, 'Campaign', return_value=self.campaign),
            patch.object(runner, 'checked_identity', side_effect=lambda command, image, source:
                         {'image': image, 'source': source, 'config': 'd' * 64}),
            patch.object(runner, 'checked_dependency', return_value={'image': 'mongo'}),
            patch.object(runner, 'checked_identity_dependency', return_value={'image': 'identity'}),
            patch.object(runner.subprocess, 'check_output', return_value='{}'),
            patch.object(runner, 'manifests', return_value=[]),
            patch.object(runner.catalog, 'infrastructure', return_value=[]),
            patch.object(runner, 'select_ready_api_pod', return_value={'uid': 'reader-uid'}),
            patch.object(runner.catalog, 'capture', side_effect=lambda *args: ({}, {})),
            patch.object(runner.catalog, 'compare', return_value={'outcome': 'PRESERVED'}),
        ]
        for mock in mocks:
            stack.enter_context(mock)

    def receipt(self):
        return json.loads((self.args.output / 'kind-evidence.private.json').read_text())

    def test_interrupt_retains_partial_receipts_and_propagates_after_cleanup(self):
        interruption = KeyboardInterrupt()
        self.campaign.fixture.side_effect = [{'stage': 'old-write'}, interruption]
        with self.assertRaises(KeyboardInterrupt) as raised:
            runner.run(self.args)
        self.assertIs(raised.exception, interruption)
        value = self.receipt()
        self.assertEqual(value['outcome'], 'CANCELLED')
        self.assertEqual(value['reason'], 'campaign interrupted: KeyboardInterrupt')
        self.assertEqual(value['fixtures'], [{'stage': 'old-write'}])
        self.assertEqual(len(value['boots']), 2)
        self.assertTrue(value['cleanup']['cluster_absent'])
        self.campaign.cleanup.assert_called_once_with()
        self.campaign.diagnostics.assert_not_called()

    def test_system_exit_zero_cannot_certify_an_incomplete_campaign(self):
        self.campaign.create.side_effect = SystemExit(0)
        with self.assertRaises(SystemExit) as raised:
            runner.run(self.args)
        self.assertEqual(raised.exception.code, 0)
        self.assertEqual(self.receipt()['outcome'], 'CANCELLED')
        self.assertEqual(self.receipt()['fixtures'], [])
        self.campaign.cleanup.assert_called_once_with()

    def test_failed_cleanup_keeps_cancellation_and_diagnostic(self):
        self.campaign.create.side_effect = KeyboardInterrupt()
        self.campaign.cleanup.side_effect = runner.RunnerError('owned cluster remains')
        with self.assertRaises(KeyboardInterrupt):
            runner.run(self.args)
        value = self.receipt()
        self.assertEqual(value['outcome'], 'CANCELLED')
        self.assertFalse(value['cleanup']['cluster_absent'])
        self.assertEqual(value['cleanup']['error'], 'owned cluster remains')

    def test_product_failure_is_blocked_and_keeps_diagnostics(self):
        self.campaign.create.side_effect = runner.RunnerError('failed boot')
        with self.assertRaisesRegex(runner.RunnerError, 'failed boot'):
            runner.run(self.args)
        self.assertEqual(self.receipt()['outcome'], 'BLOCKED')
        self.campaign.diagnostics.assert_called_once_with()
        self.campaign.cleanup.assert_called_once_with()

    def test_completed_stages_require_separate_coverage_review(self):
        result = runner.run(self.args)
        self.assertEqual(result['outcome'], 'UNVERIFIED')
        self.assertEqual(len(result['fixtures']), 4)
        self.assertEqual(len(result['boots']), 3)
        self.assertEqual(self.receipt(), result)
        self.campaign.cleanup.assert_called_once_with()

    def test_legacy_campaign_uses_distinct_boot_aliases_for_equal_artifacts(self):
        self.args.new_image = self.args.old_image
        self.args.new_source = self.args.old_source
        result = runner.run(self.args)
        self.assertEqual(result["outcome"], "UNVERIFIED")
        self.assertEqual([call.args[0] for call in self.campaign.boot.call_args_list],
                         ["old", "new", "old"])

    def test_release_probe_binds_overlays_to_declared_reader_roles(self):
        old = {"image": self.args.old_image, "source": self.args.old_source}
        new = {"image": self.args.new_image, "source": self.args.new_source}
        plan = json.loads(self.args.plan.read_text())
        plan["candidate"] = new
        plan["readers"] = [
            {"id": role, "role": role, "artifact": artifact}
            for role, artifact in zip(("historical", "current", "candidate", "recovery"),
                                      (old, new, new, old))
        ]
        self.args.plan.write_text(json.dumps(plan))
        self.args.writer_policy = "enabled"
        for overlay in ("membership_fixture", "rate_fixture"):
            with self.subTest(overlay=overlay):
                setattr(self.args, overlay, True)
                self.args.output = self.args.output.parent / overlay
                self.campaign.boot.side_effect = lambda image, source, stage, config, **control: {"stage": stage}
                with patch('migration_fixture_dependencies.configure', return_value=[{
                        'kind': 'Deployment', 'metadata': {'name': 'api'},
                        'spec': {'template': {'spec': {'containers': [{'name': 'api'}]}}}}]), \
                        patch('migration_fixture_dependencies.capture', return_value=None):
                    result = runner.run(self.args)
                self.assertEqual(len(result['boots']), 4)
                self.assertEqual(self.campaign.reader_artifacts, {role: artifact for role, artifact in
                                 zip(("historical", "current", "candidate", "recovery"), (old, new, new, old))})
                setattr(self.args, overlay, False)


if __name__ == '__main__':
    unittest.main()
