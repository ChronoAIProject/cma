from pathlib import Path
import sys
import unittest
from unittest.mock import patch

sys.path.insert(0, str(Path(__file__).resolve().parent / 'cma'))
import provider


class DeliverableProviderTests(unittest.TestCase):
    def test_marked_answer_emits_only_the_registered_references(self):
        marker = 'WS4_DELIVERY_' + 'a' * 32
        references = [f'<!--cma-deliverable:v1:{value}-->' for value in ('YQ', 'Yg', 'Yw')]
        with patch.object(provider, 'CASES', {}), patch.object(provider, 'ACTIVE', None):
            provider.arm('fixture', marker, deliverables=references)
            armed = provider.CASES['fixture']
            events = provider.response_events('response-fixture', 'synthetic', marker, armed['deliverables'])
            text = events[-1]['response']['output'][0]['content'][0]['text']
            self.assertEqual(text, f'WS4 complete {marker}\n\n' + '\n'.join(references))
            for invalid in (references[:1], ['unsafe'] * 3):
                with self.assertRaisesRegex(ValueError, 'fixture_answer_deliverables_invalid'):
                    provider.arm('invalid', marker, deliverables=invalid)
            with self.assertRaisesRegex(ValueError, 'fixture_answer_deliverables_invalid'):
                provider.arm('wrong-marker', 'WS4_other', deliverables=references)
