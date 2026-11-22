"""Immutable source comparisons fence all AM/runtime contract transitions."""
import json
from pathlib import Path
import subprocess
import tempfile
import unittest

from staging_cd_common import Refusal
from staging_cd_compatibility import ContractPolicy, GROUP, RECIPE, VERSION_FILE, runtime_labels
from staging_cd_history import DevelopHistory

LABELS = ('LABEL dev.chrono.cma.runtime-contract="3" \\\n'
          ' dev.chrono.cma.runtime-bootstrap="1" dev.chrono.cma.home-layout="2"\n')


class CompatibilityTests(unittest.TestCase):
    def setUp(self):
        directory = tempfile.TemporaryDirectory()
        self.addCleanup(directory.cleanup)
        self.root = Path(directory.name)
        self.git("init", "-q", "-b", "develop")
        self.git("config", "user.name", "Synthetic Test")
        self.git("config", "user.email", "test@example.invalid")
        self.versions = {'schema': 'cma-runtime-versions-v1', 'runtime_contract': '3', 'bootstrap': '1', 'home_layout': '2'}
        self.write(VERSION_FILE, json.dumps(self.versions))
        self.write(RECIPE, 'FROM scratch\n' + LABELS)
        self.base = self.commit()
        self.write('README.md', 'unchanged contracts\n')
        self.current = self.commit()

    def git(self, *args):
        return subprocess.check_output(['git', *args], cwd=self.root, stderr=subprocess.PIPE, text=True).strip()

    def write(self, name, value):
        path = self.root / name
        path.parent.mkdir(parents=True, exist_ok=True)
        path.write_text(value)

    def commit(self):
        self.git('add', '.')
        self.git('commit', '-qm', 'Synthetic contract input')
        return self.git('rev-parse', 'HEAD')

    def policy(self):
        head = self.git('rev-parse', 'HEAD')
        return ContractPolicy(DevelopHistory(head, self.git('rev-list', '--first-parent', head).splitlines()), self.root)

    def tasks(self, source, components=GROUP):
        return {key: [{'mode': 'publish', 'reservation': {'source': source}}] for key in components}

    def test_selector_selects_the_whole_group_for_its_contract_evidence(self):
        # A transition needs every member selected at or after it. The shared
        # selector keeps its own copy of these names; they must stay equal.
        from ci_tracks.publication_inputs import CONTRACT_FILES, CONTRACT_IMAGES
        from staging_cd_versions import DEPLOYABLES
        self.assertEqual(CONTRACT_FILES, {VERSION_FILE, RECIPE})
        self.assertEqual(CONTRACT_IMAGES, {DEPLOYABLES[key] for key in GROUP})

    def test_ordinary_source_and_packaging_changes_are_independent(self):
        policy = self.policy()
        self.assertFalse(policy.transition(self.current))
        self.assertIsNone(policy.group_reason(self.tasks(self.current), {'am': {'source': self.current}}))
        self.write(RECIPE, 'FROM scratch\nRUN echo "packaging"\n' + LABELS)
        packaged = self.commit()
        self.assertFalse(self.policy().transition(packaged))

    def test_contract_source_changes_require_every_group_member(self):
        for field in ('runtime_contract', 'bootstrap', 'home_layout'):
            with self.subTest(field=field):
                self.versions[field] = str(int(self.versions[field]) + 1)
                self.write(VERSION_FILE, json.dumps(self.versions))
                changed = self.commit()
                policy = self.policy()
                self.assertTrue(policy.transition(changed))
                self.assertEqual(policy.group_reason(self.tasks(changed), {'am': {'source': changed}}),
                                 'contract-group-incomplete')
                self.assertEqual(policy.group_reason(self.tasks(changed, {'am'}), {'am': {'source': changed}}),
                                 'contract-group-incomplete')
                receipts = {key: {'source': changed} for key in GROUP}
                self.assertIsNone(policy.group_reason(self.tasks(changed), receipts))
                receipts['runtime']['source'] = self.current
                self.assertEqual(policy.group_reason(self.tasks(changed), receipts), 'contract-group-mismatch')

    def test_version_or_bootstrap_label_changes_are_transitions(self):
        for old, new in (('contract="3"', 'contract="4"'), ('bootstrap="1"', 'bootstrap="2"')):
            with self.subTest(old=old):
                self.write(RECIPE, 'FROM scratch\n' + LABELS.replace(old, new))
                changed = self.commit()
                self.assertTrue(self.policy().transition(changed))

    def test_unknown_or_incomplete_contract_evidence_never_proves_independence(self):
        for text in ('FROM scratch\n', LABELS + LABELS.replace('contract="3"', 'contract="4"'),
                     LABELS.replace('contract="3"', 'contract="$DYNAMIC"'), 'LABEL "unterminated'):
            with self.subTest(text=text), self.assertRaises(ValueError):
                runtime_labels(text)
        self.write(VERSION_FILE, json.dumps(dict(self.versions, bootstrap='2')))
        changed = self.commit()
        policy = self.policy()
        with self.assertRaises(Refusal):
            policy.transition(self.base)
        with self.assertRaises(Refusal):
            policy.signature('f' * 40)
        (self.root / VERSION_FILE).unlink()
        missing = self.commit()
        with self.assertRaises(Refusal):
            self.policy().transition(missing)
        self.assertTrue(policy.transition(changed))

    def test_first_declaration_requires_the_whole_group_without_guessing_legacy_versions(self):
        (self.root / VERSION_FILE).unlink()
        legacy = self.commit()
        self.write(VERSION_FILE, json.dumps(self.versions))
        introduced = self.commit()
        policy = self.policy()
        self.assertTrue(policy.transition(introduced))
        with self.assertRaises(Refusal):
            policy.signature(legacy)
        self.assertEqual(policy.group_reason(self.tasks(introduced, {'am'}), {'am': {'source': introduced}}),
                         'contract-group-incomplete')

    def test_a_prior_transition_in_a_multi_source_chain_cannot_be_hidden_by_its_tail(self):
        self.write(VERSION_FILE, json.dumps(dict(self.versions, bootstrap='2')))
        changed = self.commit()
        self.write('README.md', 'later unchanged contracts\n')
        tail = self.commit()
        tasks = self.tasks(tail)
        tasks['am'].insert(0, {'mode': 'resume', 'receipt': {'source': changed}})
        self.assertEqual(self.policy().group_reason(tasks, {'am': {'source': tail}}), 'contract-group-incomplete')


if __name__ == '__main__':
    unittest.main()
