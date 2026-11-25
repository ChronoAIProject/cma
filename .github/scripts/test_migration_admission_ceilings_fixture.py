"""Admission ceilings fixture must expose lost or rewritten operator state."""
from copy import deepcopy
from pathlib import Path
import tempfile
import unittest

from migration_admission_ceilings_fixture import (
    CEILINGS, GATES, STAGES, AdmissionCeilingsFixture, manifests)


class API:
    """In-memory administration routes with newest-first history."""

    def __init__(self, output):
        self.output = output
        self.writes = True
        self.rewrite_history = False
        self.state = {'revision': 0, 'operator_stop': None, 'last_change': None,
                      'writes_available': True,
                      'ceilings': {'revision': 0, 'ceilings': {}, 'last_change': None,
                                   'writes_available': True},
                      'effective_limits': {}, 'capacity': {}, 'ceilings_history': [],
                      'history': []}

    def admin_request(self, method, route, body=None, status=200):
        state = self.state
        if method == 'GET':
            assert route == '/api/v1/admin/admission'
            view = deepcopy(state)
            view['writes_available'] = view['ceilings']['writes_available'] = self.writes
            return view
        assert self.writes
        if route.endswith('/ceilings'):
            assert body['expected_revision'] == state['ceilings']['revision']
            state['ceilings']['revision'] += 1
            state['ceilings']['ceilings'] = deepcopy(body['ceilings'])
            state['ceilings_history'].insert(0, {'revision': state['ceilings']['revision'],
                                                 'reason': body['reason']})
            if self.rewrite_history and len(state['ceilings_history']) > 1:
                state['ceilings_history'][1]['reason'] = 'rewritten'
            return {'revision': state['ceilings']['revision'],
                    'ceilings': deepcopy(body['ceilings'])}
        assert body['expected_revision'] == state['revision']
        state['revision'] += 1
        stop = route.endswith('/stop')
        state['operator_stop'] = {'reason': body['reason']} if stop else None
        state['history'].insert(0, {'revision': state['revision'], 'stop': stop})
        return {'revision': state['revision'], 'operator_stop': deepcopy(state['operator_stop'])}


class Fixture:
    def __init__(self, api, output):
        self.api, self.output = api, output

    def admin_request(self, *args, **kwargs):
        return self.api.admin_request(*args, **kwargs)


def run(api, output, stages=STAGES):
    for stage in stages:
        AdmissionCeilingsFixture(Fixture(api, output)).exercise(stage, [])


class AdmissionCeilingsFixtureTests(unittest.TestCase):
    def setUp(self):
        self.directory = tempfile.TemporaryDirectory()
        self.output = Path(self.directory.name)
        self.api = API(self.output)

    def tearDown(self):
        self.directory.cleanup()

    def test_four_stages_preserve_ceilings_and_extend_history(self):
        run(self.api, self.output)
        self.assertEqual(self.api.state['ceilings']['ceilings'], CEILINGS['new-write'])
        self.assertEqual(len(self.api.state['ceilings_history']), 2)
        self.assertEqual(len(self.api.state['history']), 4)
        self.assertIsNone(self.api.state['operator_stop'])

    def test_reader_refuses_lost_ceilings(self):
        run(self.api, self.output, STAGES[:1])
        self.api.state['ceilings']['ceilings'] = {}
        with self.assertRaisesRegex(RuntimeError, 'does not preserve'):
            run(self.api, self.output, STAGES[1:2])

    def test_writer_refuses_rewritten_history(self):
        run(self.api, self.output, STAGES[:2])
        self.api.rewrite_history = True
        with self.assertRaisesRegex(RuntimeError, 'history was rewritten'):
            run(self.api, self.output, STAGES[2:3])

    def test_writer_refuses_gated_writes_and_prior_state(self):
        self.api.writes = False
        with self.assertRaisesRegex(RuntimeError, 'gated off'):
            run(self.api, self.output, STAGES[:1])
        self.api.writes = True
        self.api.state['ceilings']['revision'] = 3
        with self.assertRaisesRegex(RuntimeError, 'unexpected prior'):
            run(self.api, self.output, STAGES[:1])

    def test_overlay_enables_both_gates_once(self):
        objects = [{'kind': 'ConfigMap', 'metadata': {'name': 'api-env'}, 'data': {}}]
        manifests(objects)
        self.assertEqual({gate: objects[0]['data'][gate] for gate in GATES},
                         {gate: 'true' for gate in GATES})
        with self.assertRaisesRegex(RuntimeError, 'unconfigured'):
            manifests(objects)


if __name__ == '__main__':
    unittest.main()
