import json
from pathlib import Path
import tempfile
import unittest

from stage_evidence import fields, summarize


class StageEvidenceTests(unittest.TestCase):
    def test_text_and_json_fields_keep_generic_conflict_unclassified(self):
        examples = [
            '2026-11-16T00:00:00Z ERROR agent_scheduler_failure: failed phase="execution" failure="conflict_unclassified" agent_hash="private-test"',
            json.dumps({'target': 'agent_scheduler_failure', 'fields': {
                'phase': 'execution', 'failure': 'conflict_unclassified', 'private': 'omitted'}}),
        ]
        for example in examples:
            value = fields(example)
            self.assertEqual(value['phase'], 'execution')
            self.assertEqual(value['failure'], 'conflict_unclassified')
        self.assertIsNone(fields('record revision changed owner lease failure'))

    def test_summary_never_exports_payload_identity_or_unproven_cause(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            events = [
                {'phase': 'execution', 'failure': 'conflict_unclassified', 'agent_hash': 'private-identity'},
                {'phase': 'execution', 'failure': 'unknown-raw-driver-payload'},
                {'phase': 'private-user-id', 'failure': 'task_panicked'},
            ]
            raw = '\n'.join(json.dumps({'target': 'agent_scheduler_failure', 'fields': event}) for event in events)
            (root / 'am-a-0.full.private.log').write_text(raw)
            (root / 'am-b-0.full.private.log').write_text('private unstructured message record revision changed')
            nested = root / 'older-capture'; nested.mkdir()
            (nested / 'am-a-0.full.private.log').write_text(raw)
            result = summarize(root)
            self.assertEqual(result['occurrences'], [{'am': 'a', 'phase': 'execution',
                'failure': 'conflict_unclassified', 'count': 1}])
            self.assertEqual(result['unrecognized_scheduler_events'], 2)
            self.assertEqual(len(result['files']), 2)
            self.assertFalse(result['root_cause_inferred'])
            self.assertNotIn('private', json.dumps(result))
            self.assertNotIn('raw-driver', json.dumps(result))

    def test_no_log_does_not_claim_zero_incident_or_diagnostics_pass(self):
        with tempfile.TemporaryDirectory() as temporary:
            result = summarize(Path(temporary))
            self.assertEqual(result['files'], [])
            self.assertFalse(result['complete_history'])
            self.assertNotIn('verdict', result)
