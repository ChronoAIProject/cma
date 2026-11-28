import json
import unittest

from setup_evidence import capture, summarize_pods


class SetupEvidenceTests(unittest.TestCase):
    def test_status_summary_omits_identity_messages_and_unknown_reasons(self):
        pod = {'metadata': {'name': 'private-owner'}, 'status': {
            'phase': 'Pending', 'containerStatuses': [
                {'name': 'private', 'state': {'waiting': {'reason': 'CrashLoopBackOff',
                    'message': 'private credential and runtime response'}}},
                {'state': {'terminated': {'reason': 'private message', 'exitCode': 1}}},
                {'state': {'waiting': {'reason': []}}},
            ]}}
        result = summarize_pods({'items': [pod]})
        self.assertEqual(result['pods'], 1)
        self.assertEqual(result['phases'], {'Pending': 1})
        self.assertIn({'state': 'waiting', 'reason': 'CrashLoopBackOff', 'exit_code': None,
                       'count': 1}, result['container_states'])
        self.assertNotIn('private', json.dumps(result))

    def test_capture_binds_owned_node_and_reads_only_fixed_nonsecret_resources(self):
        class Fixture:
            bound = False
            calls = []
            def bind_node(self):
                self.bound = True
            def k(self, args, **kwargs):
                assert self.bound
                self.calls.append((args, kwargs))
                if 'logs' in args:
                    return 'private logs never copied to public summary'
                if 'events' in args:
                    return json.dumps({'items': [{'reason': 'Failed', 'message': 'private'}]})
                raise RuntimeError('private command failure')
        fixture = Fixture()
        result = capture(fixture)
        self.assertEqual(len(fixture.calls), 6)
        for args, kwargs in fixture.calls:
            self.assertEqual(args[0], '-n')
            self.assertIn(args[1], ['agent-sandbox-system', 'cma-native-storage'])
            self.assertIn(args[2], ['get', 'logs'])
            self.assertEqual(kwargs['timeout'], 35)
        self.assertTrue(result['namespaces']['storage']['pods_capture_failed'])
        self.assertEqual(result['namespaces']['storage']['event_reasons'], {'Failed': 1})
        self.assertNotIn('private', json.dumps(result))

    def test_changed_node_stops_capture_before_any_kubernetes_call(self):
        class Foreign:
            def bind_node(self):
                raise ValueError('kind_node_identity_changed')
            def k(self, *_args, **_kwargs):
                self.fail('must not read foreign cluster')
        with self.assertRaisesRegex(ValueError, 'kind_node_identity_changed'):
            capture(Foreign())
