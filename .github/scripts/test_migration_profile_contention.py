from contextlib import contextmanager
from copy import deepcopy
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
import json
from pathlib import Path
from threading import Barrier, Lock, Thread
import tempfile
import unittest
from unittest.mock import patch

import migration_profile_contention as contention
from migration_profile_fixture import Fixture, RateLimited


def profile(kind):
    draft = {'agent': {'model': 'model-fixture', 'system_prompt': 'original'},
             'workspace': {'name': 'original', 'agent_profiles': [{'id': 'ap_fixture', 'revision': 1}]},
             'sandbox': {'environment_variables': [],
                         'workspace_profiles': [{'id': 'wp_fixture', 'revision': 1}]}}[kind]
    return {'id': kind + '_fixture', 'draft_version': 1, 'draft': draft,
            'name': 'Original', 'description': 'Original description', 'owner': 'fixture-owner',
            'visibility': 'private', 'created_at': '2026-11-17T00:00:00Z',
            'latest_revision': None}


@contextmanager
def endpoint(kind, failure=None):
    state = {'current': profile(kind), 'completed': {}, 'initial_requests': 0,
             'received': [], 'failure': failure}
    lock, barrier = Lock(), Barrier(2, timeout=5)

    class Handler(BaseHTTPRequestHandler):
        def log_message(self, *_args):
            pass

        def respond(self, code, value):
            raw = json.dumps(value).encode()
            self.send_response(code)
            self.send_header('Content-Type', 'application/json')
            self.send_header('Content-Length', str(len(raw)))
            self.send_header('x-request-id', 'fixture-request')
            self.end_headers()
            self.wfile.write(raw)

        def do_GET(self):
            with lock:
                current = deepcopy(state['current'])
            self.respond(200, current)

        def do_PUT(self):
            body = json.loads(self.rfile.read(int(self.headers['Content-Length'])))
            key = self.headers['Idempotency-Key']
            with lock:
                state['received'].append({'key': key, 'body': body})
                initial = key not in state['completed'] and state['initial_requests'] < 2
                if initial:
                    state['initial_requests'] += 1
            # This server barrier requires both HTTP requests to arrive before
            # either can complete. A sequential fixture fails the test.
            if initial:
                barrier.wait()
            with lock:
                failure = state['failure']
                if key in state['completed']:
                    code, value = deepcopy(state['completed'][key])
                    if failure == 'replay_mutation':
                        state['current']['draft_version'] += 1
                    if failure == 'replay_response':
                        value['unexpected'] = True
                elif failure in ('http_500', 'http_429'):
                    code, value = (500 if failure == 'http_500' else 429), {'code': 'unavailable'}
                elif failure == 'both_accept' or body['draft_version'] == state['current']['draft_version']:
                    state['current'].update(name=body['name'], description=body['description'],
                                            draft=body['content'], draft_version=body['draft_version'] + 1)
                    code, value = 200, deepcopy(state['current'])
                    if failure == 'wrong_identity':
                        value['id'] = 'wrong'
                else:
                    code, value = 409, {'code': 'profile_version_conflict'}
                    if failure == 'wrong_conflict':
                        value['code'] = 'idempotency_key_reused'
                    if failure == 'stale_mutation' and key.startswith('migration-stale-'):
                        state['current']['name'] = body['name']
                state['completed'][key] = code, deepcopy(value)
            self.respond(code, value)

    server = ThreadingHTTPServer(('127.0.0.1', 0), Handler)
    thread = Thread(target=server.serve_forever, kwargs={'poll_interval': 0.01})
    thread.start()
    fixture = Fixture.__new__(Fixture)
    fixture.base = 'http://127.0.0.1:' + str(server.server_port)
    fixture.cookies, fixture.requests = {}, []
    fixture.last_retry_after, fixture.last_create_request_at = None, None
    try:
        yield fixture, state
    finally:
        server.shutdown()
        server.server_close()
        thread.join(timeout=5)
        if thread.is_alive():
            raise RuntimeError('profile fixture server did not join')


class ProfileContentionTests(unittest.TestCase):
    def test_three_profile_kinds_race_and_replay_through_http(self):
        for kind in ('agent', 'workspace', 'sandbox'):
            with self.subTest(kind=kind), endpoint(kind) as (fixture, state):
                before = deepcopy(state['current'])
                after, receipt = contention.race(fixture, kind, '/profiles/test', before)
                self.assertEqual(sorted(command['status'] for command in receipt['commands']), [200, 409])
                self.assertEqual(after['draft_version'], 2)
                self.assertEqual(state['initial_requests'], 2)
                self.assertNotEqual(after['draft'], before['draft'])
                self.assertEqual(state['current'], after)
                self.assertLess(max(c['started_at'] for c in receipt['commands']),
                                min(c['finished_at'] for c in receipt['commands']))
                initial_keys = {command['key'] for command in receipt['commands']}
                self.assertEqual(sum(request['key'] in initial_keys for request in state['received']), 4)

    def test_other_reader_replays_original_response_after_publication_and_visibility_change(self):
        with endpoint('agent') as (fixture, state):
            after, receipt = contention.race(fixture, 'agent', '/profiles/test', deepcopy(state['current']))
            state['current']['visibility'] = 'public'
            state['current']['latest_revision'] = {'revision': 1, 'digest': 'retained-digest'}
            current = deepcopy(state['current'])
            # A fresh HTTP client represents a new reader process using the
            # retained application state; this is harness validation only.
            other = Fixture.__new__(Fixture)
            other.base, other.cookies, other.requests = fixture.base, {}, []
            other.last_retry_after, other.last_create_request_at = None, None
            contention.verify(other, json.loads(json.dumps(receipt)), current)
            self.assertEqual(state['current'], current)
            self.assertEqual(next(c for c in receipt['commands'] if c['status'] == 200)['response'], after)

    def test_two_accepted_writes_or_wrong_conflict_cannot_pass(self):
        for failure in ('both_accept', 'wrong_conflict', 'wrong_identity'):
            with self.subTest(failure=failure), endpoint('agent', failure) as (fixture, state):
                with self.assertRaises(RuntimeError):
                    contention.race(fixture, 'agent', '/profiles/test', deepcopy(state['current']))

    def test_replay_mutation_response_change_and_stale_overwrite_fail(self):
        for failure in ('replay_mutation', 'replay_response', 'stale_mutation'):
            with self.subTest(failure=failure), endpoint('workspace', failure) as (fixture, state):
                with self.assertRaises(RuntimeError):
                    contention.race(fixture, 'workspace', '/profiles/test', deepcopy(state['current']))

    def test_transport_status_failures_have_no_automatic_retry(self):
        for failure in ('http_500', 'http_429'):
            with self.subTest(failure=failure), endpoint('sandbox', failure) as (fixture, state):
                with self.assertRaises(RuntimeError):
                    contention.race(fixture, 'sandbox', '/profiles/test', deepcopy(state['current']))
                self.assertEqual(len(state['received']), 2)
                self.assertEqual(len({row['key'] for row in state['received']}), 2)

    def test_invalid_input_and_incomplete_retained_receipt_fail_before_http(self):
        for kind, before in [('unknown', profile('agent')), ('agent', {}),
                             ('agent', {**profile('agent'), 'draft_version': True})]:
            with self.assertRaises(RuntimeError):
                contention.intent(kind, before, 'one')
        with endpoint('agent') as (fixture, state):
            with self.assertRaises(RuntimeError):
                contention.verify(fixture, {'path': '/profiles/test', 'commands': []}, state['current'])
            self.assertEqual(state['received'], [])

    def test_missing_or_nonoverlapping_request_timing_cannot_certify_a_race(self):
        for invalid in ('missing', 'sequential'):
            with self.subTest(invalid=invalid), endpoint('agent') as (fixture, state):
                original = fixture.request

                def request(*args, **kwargs):
                    response = original(*args, **kwargs)
                    if args[0] == 'PUT':
                        row = next(r for r in fixture.requests
                                   if r['idempotency_key'] == kwargs['key'])
                        if invalid == 'missing':
                            row.pop('started_at')
                        else:
                            start = 1 if kwargs['body']['name'].endswith('left') else 3
                            row.update(started_at=start, finished_at=start + 1)
                    return response

                with patch.object(fixture, 'request', side_effect=request):
                    with self.assertRaisesRegex(RuntimeError, 'timing|did not overlap'):
                        contention.race(fixture, 'agent', '/profiles/test', deepcopy(state['current']))

    def test_profile_creation_uses_the_winning_version_and_preserves_the_race_receipt(self):
        fixture = Fixture.__new__(Fixture)
        before, edited = profile('agent'), profile('agent')
        edited['draft_version'] = 2
        edited['draft']['system_prompt'] = 'winner'
        revision = {'content': edited['draft'], 'digest': 'digest', 'profile_id': before['id']}
        current = {**edited, 'latest_revision': {'digest': 'digest'}}
        with patch.object(fixture, 'request', side_effect=[before, before, revision, current]) as request, \
                patch.object(contention, 'race', return_value=(edited, {'commands': 'receipt'})):
            row = fixture.create('agent', {'content': before['draft']})
        self.assertEqual(row['publish_request'], {'draft_version': 2})
        self.assertEqual(row['edit_receipt'], {'commands': 'receipt'})
        self.assertEqual(request.call_count, 4)
        with patch.object(fixture, 'request', side_effect=RateLimited('POST', '/profiles', 65)) as request:
            with self.assertRaises(RateLimited):
                fixture.create('sandbox', {})
            self.assertEqual(request.call_count, 1)

    def test_reader_checks_published_revision_after_replaying_updates(self):
        with tempfile.TemporaryDirectory() as directory:
            fixture = Fixture.__new__(Fixture)
            fixture.output = Path(directory)
            row = {'path': '/profiles/test', 'current': profile('agent'),
                   'revision': {'revision': 1, 'digest': 'immutable'},
                   'edit_receipt': {'commands': 'receipt'}}
            for stage, writer in [('new-read-old-write', 'old-write'), ('old-read-new-write', 'new-write')]:
                (fixture.output / (writer + '-profiles.private.json')).write_text(json.dumps([row]))
                with patch.object(contention, 'verify') as verify, patch.object(fixture, 'request',
                        side_effect=[row['current'], row['revision'], row['revision']]):
                    self.assertEqual(fixture.read(stage), [row])
                verify.assert_called_once_with(fixture, row['edit_receipt'], row['current'])
            with patch.object(contention, 'verify'), patch.object(fixture, 'request',
                    side_effect=[row['current'], row['revision'], {'revision': 2}]):
                with self.assertRaisesRegex(AssertionError, 'published revision'):
                    fixture.read('new-read-old-write')


if __name__ == '__main__':
    unittest.main()
