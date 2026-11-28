import json
import sys
from pathlib import Path
import unittest
from unittest.mock import patch

sys.path.insert(0, str(Path(__file__).resolve().parent / 'cma'))
import provider


class SkillProviderTests(unittest.TestCase):
    skill = 'two-am-after-restart'

    def message(self, content, role='developer'):
        return {'role': role, 'content': [{'type': 'input_text', 'text': content}]}

    def catalog(self, *names):
        rows = '\n'.join(f'- {name}: Synthetic description (file: /fixture/{name}/SKILL.md)' for name in names)
        return self.message('<skills_instructions>\n' + rows + '\n</skills_instructions>')

    def test_requires_a_catalog_and_considers_only_the_current_developer_catalog(self):
        absent = {'id': self.skill, 'present': False}
        present = {'id': self.skill, 'present': True}
        old, new = self.catalog('baseline'), self.catalog('baseline', self.skill)
        self.assertFalse(provider.skill_matches(absent, {'input': []}))
        self.assertTrue(provider.skill_matches(absent, {'input': [new, old]}))
        self.assertTrue(provider.skill_matches(present, {'input': [old, new]}))
        self.assertFalse(provider.skill_matches(present, {'input': [old, self.message(self.skill, 'user')]}))
        self.assertFalse(provider.skill_matches(present, {'input': [self.catalog(self.skill, self.skill)]}))
        self.assertFalse(provider.skill_matches(present, {'input': [self.catalog('other-' + self.skill)]}))

    def test_control_snapshot_exposes_only_the_count_and_rejects_other_skill_names(self):
        with patch.object(provider, 'CASES', {}), patch.object(provider, 'ACTIVE', None):
            provider.arm('skill-case', 'WS4_turn_marker', skill={'id': self.skill, 'present': True})
            snapshot = provider.snapshot('skill-case')
            self.assertEqual(snapshot['skill_matches'], 0)
            self.assertNotIn(self.skill, json.dumps(snapshot))
            with self.assertRaisesRegex(ValueError, 'fixture_skill_invalid'):
                provider.arm('other-skill', 'WS4_turn_marker', skill={'id': 'external', 'present': True})
