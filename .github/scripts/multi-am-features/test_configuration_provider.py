import json
import sys
from pathlib import Path
import unittest
from unittest.mock import patch

sys.path.insert(0, str(Path(__file__).resolve().parent / 'cma'))
import provider


class ConfigurationProviderTests(unittest.TestCase):
    def setUp(self):
        self.marker = 'WS4_INSTRUCTIONS_' + 'a' * 32
        self.expected = {'effort': 'medium', 'instructions': self.marker}

    def test_requires_exact_changed_effort_and_a_native_instruction_location(self):
        body = {'reasoning': {'effort': 'medium'}}
        line = 'Two-AM instructions ' + self.marker
        def input_message(text):
            return {'input': [{'type': 'message', 'role': 'user',
                               'content': [{'type': 'input_text', 'text': text}]}]}

        context = input_message('# AGENTS.md instructions for fixture\n<INSTRUCTIONS>\n' + line
                                + '\n</INSTRUCTIONS>')
        self.assertTrue(provider.configuration_matches(self.expected, {**body, **context}))
        self.assertTrue(provider.configuration_matches({'effort': 'medium'}, body))
        self.assertFalse(provider.configuration_matches(self.expected, {
            **body, **input_message(line),
        }))
        self.assertFalse(provider.configuration_matches(self.expected, {
            **body, **input_message('# AGENTS.md instructions for fixture\n<INSTRUCTIONS>\n</INSTRUCTIONS>\n' + line),
        }))
        self.assertFalse(provider.configuration_matches(self.expected, {'reasoning': {'effort': 'high'}, **context}))
        self.assertFalse(provider.configuration_matches(self.expected, context))

    def test_control_snapshot_keeps_probe_payload_and_expected_marker_private(self):
        with patch.object(provider, 'CASES', {}), patch.object(provider, 'ACTIVE', None):
            provider.arm('configuration-case', 'WS4_turn_marker', configuration=self.expected)
            snapshot = provider.snapshot('configuration-case')
            self.assertEqual(snapshot['configuration_matches'], 0)
            self.assertNotIn(self.marker, json.dumps(snapshot))
            with self.assertRaisesRegex(ValueError, 'fixture_configuration_invalid'):
                provider.arm('bad-marker', 'WS4_turn_marker', configuration={
                    'effort': 'medium', 'instructions': 'arbitrary prompt',
                })
            with self.assertRaisesRegex(ValueError, 'fixture_configuration_invalid'):
                provider.arm('bad-effort', 'WS4_turn_marker', configuration={'effort': {}})
