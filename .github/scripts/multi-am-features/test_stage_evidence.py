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

    def test_malformed_typed_fields_do_not_drop_later_valid_events(self):
        for name in ['phase', 'failure']:
            for invalid in [None, True, 7, 1.5, [], {}, ['private-value'],
                            {'private-key': 'private-value'}]:
                with self.subTest(name=name, invalid=invalid):
                    valid = {'phase': 'execution', 'failure': 'task_cancelled'}
                    malformed = dict(valid, **{name: invalid})
                    raw = '\n'.join(json.dumps({'target': 'agent_scheduler_failure',
                                               'fields': event})
                                    for event in [valid, malformed, valid])
                    with tempfile.TemporaryDirectory() as temporary:
                        root = Path(temporary)
                        (root / 'am-a-0.full.private.log').write_text(raw)
                        result = summarize(root)
                    self.assertEqual(result['unrecognized_scheduler_events'], 1)
                    self.assertEqual(result['occurrences'], [{'am': 'a',
                        'phase': 'execution', 'failure': 'task_cancelled', 'count': 2}])
                    self.assertNotIn('private', json.dumps(result))

    def test_fields_projects_only_stage_and_failure(self):
        event = {'target': 'agent_scheduler_failure', 'fields': {
            'phase': 'discovery', 'failure': 'store_unavailable',
            'message': 'private-error', 'agent_hash': 'private-identity',
            'nested': {'private': 'payload'}},
            'span': {'private_identity': 'private-caller'}}
        self.assertEqual(fields(json.dumps(event)), {
            'phase': 'discovery', 'failure': 'store_unavailable'})

    def test_malformed_json_is_isolated_from_following_event(self):
        valid = json.dumps({'target': 'agent_scheduler_failure', 'fields': {
            'phase': 'reconciliation', 'failure': 'conflict_unclassified'}})
        malformed = [
            '{"target":"agent_scheduler_failure",',
            '{"private":' + '[' * 2000 + '0' + ']' * 2000 + '}',
            json.dumps({'target': 'another_target', 'fields': {
                'phase': 'execution', 'failure': 'task_panicked'}}),
        ]
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            (root / 'am-b-0.full.private.log').write_text('\n'.join(malformed + [valid]))
            result = summarize(root)
        self.assertEqual(result['occurrences'], [{'am': 'b', 'phase': 'reconciliation',
            'failure': 'conflict_unclassified', 'count': 1}])
        # Invalid JSON cannot prove the emitting target and is not a typed event.
        self.assertEqual(result['unrecognized_scheduler_events'], 0)
        self.assertFalse(result['complete_history'])
        self.assertNotIn('private', json.dumps(result))

    def test_invalid_fields_container_is_rejected_as_one_scheduler_event(self):
        for invalid in [None, [], 'private-payload', 42]:
            with self.subTest(invalid=invalid):
                with tempfile.TemporaryDirectory() as temporary:
                    root = Path(temporary)
                    (root / 'am-a-0.full.private.log').write_text(json.dumps({
                        'target': 'agent_scheduler_failure', 'fields': invalid}))
                    result = summarize(root)
                self.assertEqual(result['unrecognized_scheduler_events'], 1)
                self.assertEqual(result['occurrences'], [])
                self.assertNotIn('private', json.dumps(result))
