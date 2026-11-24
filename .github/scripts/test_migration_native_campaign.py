from copy import deepcopy
import secrets
import unittest

from migration_native_campaign import NativeCampaign, NativeCampaignError, artifact, digest, stage_binding
from migration_session_child import STAGES


def artifacts():
    return {role: {'source': digit * 40, 'config': 'sha256:' + digit * 64,
                   'image': 'registry.test/cma@sha256:' + digit * 64}
            for role, digit in [('old', '1'), ('new', '2'), ('runtime', '3')]}


def binding(stage):
    index = STAGES.index(stage)
    incarnation = 0 if index == 0 else (2 if index == 3 else 1)
    role = 'old' if index in (0, 3) else 'new'
    uid = 'pod-' + str(incarnation)
    return {'stage': stage, 'origin': 'http://127.0.0.1:' + str(14000 + incarnation),
            'reader': {'artifact': artifacts()[role], 'podName': 'api', 'podUid': uid},
            'boot': None if index == 2 else {
                'schema': 'cma-migration-boot-observation-v1', 'failed_boots': 0,
                'selected_uid': uid, 'attempted_uids': [uid], 'ready_uids': [uid],
                'observations': [{'uid': uid}]}}


class Infrastructure:
    def __init__(self, actions):
        self.actions = actions
        self.failure = None
        self.observations = 0
        self.change_at = None
        self.forward_live = False
        self.prepare_started = False
        self.document_override = None
        self.binding_override = None
        self.credential = secrets.token_urlsafe(24)

    def action(self, name):
        self.actions.append(name)
        if name == self.failure:
            raise RuntimeError('private transport body with credentials')

    def prepare(self):
        self.prepare_started = True
        self.action('prepare')

    def bind_reader(self, role, stage):
        self.action('bind:' + stage)
        self.forward_live = True
        value = binding(stage)
        if self.binding_override:
            self.binding_override(value)
        return value

    def session_config(self, value):
        self.action('session_config')
        return {'artifacts': artifacts(), 'origin': value['origin'], 'bearer': self.credential}

    def observe_reader(self, value):
        self.action('observe')
        self.observations += 1
        return {**value['reader'], 'incarnation': 999 if self.observations == self.change_at else 0}

    def capture_documents(self, stage, value, _result):
        self.action('capture:' + stage)
        receipt = {'stage': stage, 'artifact': value['reader']['artifact'],
                   'reader_sha256': digest({**value['reader'], 'incarnation': 0}),
                   'outcome': 'DOCUMENTS_OBSERVED', 'compatibility': 'NOT_PROVEN',
                   'raw_sha256': 'a' * 64}
        if self.document_override:
            self.document_override(receipt)
        return receipt

    def teardown(self):
        self.action('teardown')
        self.forward_live = False
        return {'outcome': 'OWNED_INFRASTRUCTURE_REMOVED'}


class Session:
    def __init__(self, infrastructure):
        self.infrastructure = infrastructure
        self.actions = infrastructure.actions
        self.commands = []
        self.failure = None
        self.result_override = None
        self.finality_override = None

    def start(self):
        self.actions.append('start_session')
        if self.failure == 'start': raise RuntimeError('private start failure')

    def command(self, op, payload):
        self.actions.append('session:' + op)
        self.commands.append((op, deepcopy(payload)))
        if self.failure == op: raise RuntimeError('private session failure')
        if op != 'stage': return {'op': op}
        value = {'result': {'schema': 'migration-agent-session-stage-v1', 'stage': payload['stage'],
                           'result': {'artifact': payload['reader']['artifact'],
                                      'outcome': 'API_JOURNEY_OBSERVED',
                                      'persisted_collection_coverage': 'NOT_PROVEN',
                                      'full_reader_writer_gate': 'NOT_PROVEN'}}}
        if self.result_override: self.result_override(value)
        return value

    def close(self):
        self.actions.append('close_session')
        assert self.infrastructure.forward_live
        if self.failure == 'close': raise RuntimeError('private cleanup failure')
        value = {'outcome': 'SESSION_OBSERVED', 'exit_code': 0,
                 'processes': {'process_group_cleanup': True}}
        if self.finality_override: self.finality_override(value)
        return value


class CampaignTests(unittest.TestCase):
    def fixture(self):
        actions, events = [], []
        infrastructure = Infrastructure(actions)
        session = Session(infrastructure)
        def record(event):
            actions.append('record:' + event['type'])
            events.append(deepcopy(event))
        campaign = NativeCampaign(artifacts(), infrastructure, record, session_factory=lambda: session)
        return campaign, infrastructure, session, events, actions

    def test_cleanup_hook_precedes_session_and_infrastructure_cleanup_even_when_it_fails(self):
        for failure in (None, RuntimeError('hook failure'), KeyboardInterrupt()):
            campaign, _, _, _, actions = self.fixture()
            def hook():
                actions.append('cleanup_hook')
                if failure is not None: raise failure
            campaign.on_cleanup = hook
            if isinstance(failure, KeyboardInterrupt):
                with self.assertRaises(KeyboardInterrupt): campaign.run(enabled=True)
            else:
                result = campaign.run(enabled=True)
                self.assertEqual(result['outcome'], 'INCOMPLETE' if failure else 'NATIVE_CAMPAIGN_OBSERVED')
            self.assertLess(actions.index('cleanup_hook'), actions.index('close_session'))
            self.assertLess(actions.index('close_session'), actions.index('teardown'))

    def test_same_session_across_all_stages_and_finally_ordering(self):
        campaign, infra, session, events, actions = self.fixture()
        result = campaign.run(enabled=True)
        self.assertEqual(result['outcome'], 'NATIVE_CAMPAIGN_OBSERVED')
        self.assertEqual(actions.count('start_session'), 1)
        self.assertEqual([cmd[0] for cmd in session.commands], ['initialize'] + ['stage'] * 4 + ['finish'])
        self.assertEqual([row['stage'] for row in result['stages']], list(STAGES))
        self.assertEqual(infra.observations, 12)
        self.assertLess(actions.index('close_session'), actions.index('teardown'))
        self.assertFalse(infra.forward_live)
        self.assertEqual(events[-1]['type'], 'campaign_finality')
        self.assertNotIn(infra.credential, str(events))
        self.assertEqual(result['full_reader_writer_gate'], 'NOT_PROVEN')
        with self.assertRaisesRegex(NativeCampaignError, 'once'): campaign.run(enabled=True)

    def test_default_off_has_no_side_effects(self):
        campaign, _, _, events, actions = self.fixture()
        for enabled in (False, None, 1, 'true'):
            with self.assertRaisesRegex(NativeCampaignError, 'opt_in'): campaign.run(enabled=enabled)
        self.assertEqual(actions, [])
        self.assertEqual(events, [])
        self.assertEqual(campaign.run(enabled=True)['outcome'], 'NATIVE_CAMPAIGN_OBSERVED')

    def test_infrastructure_failures_do_not_restart_and_always_teardown(self):
        for failure in ('prepare', 'session_config', 'observe', 'bind:old-write',
                        'bind:new-read-old-write', 'capture:old-write', 'teardown'):
            with self.subTest(failure=failure):
                campaign, infra, session, events, actions = self.fixture()
                infra.failure = failure
                result = campaign.run(enabled=True)
                self.assertEqual(result['outcome'], 'INCOMPLETE')
                self.assertEqual(actions.count('teardown'), 1)
                self.assertLessEqual(actions.count('start_session'), 1)
                self.assertNotIn('private', str(result))
                self.assertEqual(len([r for r in session.commands if r[0] == 'initialize']),
                                 actions.count('start_session'))

    def test_session_failures_keep_forwards_through_cleanup(self):
        for failure in ('start', 'initialize', 'stage', 'finish', 'close'):
            with self.subTest(failure=failure):
                campaign, _, session, _, actions = self.fixture()
                session.failure = failure
                result = campaign.run(enabled=True)
                self.assertEqual(result['outcome'], 'INCOMPLETE')
                self.assertEqual(actions.count('close_session'), 1)
                self.assertEqual(actions.count('teardown'), 1)
                self.assertLess(actions.index('close_session'), actions.index('teardown'))
                self.assertNotIn('private', str(result))

    def test_reader_change_after_session_or_during_document_capture_stops(self):
        for position in (2, 3, 5, 6):
            campaign, infra, session, _, _ = self.fixture()
            infra.change_at = position
            result = campaign.run(enabled=True)
            self.assertEqual(result['outcome'], 'INCOMPLETE')
            self.assertTrue(any('reader_changed' in error for error in result['failures']))
            self.assertLess(len(session.commands), 6)

    def test_failed_boot_is_saved_and_cannot_be_hidden_by_cleanup(self):
        campaign, infra, session, events, _ = self.fixture()
        def failure(value):
            if value['stage'] == STAGES[3]: value['boot']['failed_boots'] = 1
        infra.binding_override = failure
        result = campaign.run(enabled=True)
        self.assertEqual(len(result['stages']), 3)
        self.assertIn('campaign_clean_boot_required', result['failures'])
        saved = [event['binding'] for event in events if event['type'] == 'reader_binding_observed']
        self.assertEqual(saved[-1]['boot']['failed_boots'], 1)
        self.assertEqual(len([cmd for cmd in session.commands if cmd[0] == 'stage']), 3)

    def test_bound_document_and_session_receipts_required(self):
        for field, value in [('stage', 'other'), ('artifact', artifacts()['runtime']),
                             ('reader_sha256', 'b' * 64), ('outcome', 'PASS'), ('compatibility', 'PASS')]:
            campaign, infra, _, _, _ = self.fixture()
            infra.document_override = lambda row: row.update({field: value})
            self.assertIn('campaign_document_binding_invalid', campaign.run(enabled=True)['failures'])
        campaign, _, session, _, _ = self.fixture()
        session.result_override = lambda row: row['result']['result'].update(full_reader_writer_gate='PASS')
        self.assertIn('campaign_session_observation_invalid', campaign.run(enabled=True)['failures'])

    def test_cleanup_failure_cannot_create_pass(self):
        for field, value in [('outcome', 'INCOMPLETE'), ('exit_code', 1), ('processes', {})]:
            campaign, _, session, _, _ = self.fixture()
            session.finality_override = lambda row: row.update({field: value})
            result = campaign.run(enabled=True)
            self.assertIn('campaign_session_incomplete', result['failures'])
            self.assertEqual(result['outcome'], 'INCOMPLETE')

    def test_record_failure_still_disposes_and_final_record_failure_raises(self):
        for fail_type in ('campaign_started', 'session_stage_intent', 'session_finality', 'campaign_finality'):
            campaign, _, _, _, actions = self.fixture()
            original = campaign.record
            def record(event):
                original(event)
                if event['type'] == fail_type: raise OSError('disk full')
            campaign.record = record
            if fail_type == 'campaign_finality':
                with self.assertRaises(OSError): campaign.run(enabled=True)
            else:
                self.assertEqual(campaign.run(enabled=True)['outcome'], 'INCOMPLETE')
            self.assertEqual(actions.count('teardown'), 1)

    def test_binding_and_artifact_validation(self):
        for value in (None, {}, {**artifacts()['old'], 'source': 'short'},
                      {**artifacts()['old'], 'config': 'tag'}, {**artifacts()['old'], 'image': 'repo:tag'}):
            with self.assertRaises(NativeCampaignError): artifact(value)
        for values in ({}, {**artifacts(), 'new': artifacts()['old']}):
            with self.assertRaises(NativeCampaignError): NativeCampaign(values, None, None)
        for mutate in (lambda v: v.update(stage='wrong'), lambda v: v.update(origin='https://prod.test'),
                       lambda v: v.update(origin='http://127.0.0.1:99999'),
                       lambda v: v['reader'].update(artifact=artifacts()['new']),
                       lambda v: v['boot'].update(ready_uids=[])):
            value = binding(STAGES[0]); mutate(value)
            with self.assertRaises(NativeCampaignError): stage_binding(value, STAGES[0], artifacts()['old'], None)
        value = binding(STAGES[2]); value['reader']['podUid'] = 'replacement'
        with self.assertRaisesRegex(NativeCampaignError, 'without_boot'):
            stage_binding(value, STAGES[2], artifacts()['new'], binding(STAGES[1]))
        value = binding(STAGES[3]); value['reader']['podUid'] = 'pod-1'
        value['boot'].update(selected_uid='pod-1', attempted_uids=['pod-1'], ready_uids=['pod-1'])
        with self.assertRaisesRegex(NativeCampaignError, 'uid_reused'):
            stage_binding(value, STAGES[3], artifacts()['old'], binding(STAGES[2]))

    def test_interruption_records_failure_and_disposes_before_reraising(self):
        campaign, infra, _, events, actions = self.fixture()
        def interrupt(_stage, _binding, _result):
            raise KeyboardInterrupt()
        infra.capture_documents = interrupt
        with self.assertRaises(KeyboardInterrupt): campaign.run(enabled=True)
        self.assertEqual(actions.count('close_session'), 1)
        self.assertEqual(actions.count('teardown'), 1)
        self.assertEqual(events[-1]['receipt']['outcome'], 'INCOMPLETE')
        self.assertIn('campaign_interrupted', events[-1]['receipt']['failures'])


if __name__ == '__main__':
    unittest.main()
