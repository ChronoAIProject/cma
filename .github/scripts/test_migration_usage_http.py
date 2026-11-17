#!/usr/bin/env python3
"""Local HTTP contract tests for the fixture, not artifact compatibility proof."""
from copy import deepcopy
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
import json
import os
from pathlib import Path
import secrets
import tempfile
import threading
import unittest
from unittest.mock import patch
from urllib.parse import urlsplit
import uuid

from migration_profile_fixture import Fixture, RateLimited
from migration_usage_fixture import ROUTE, UsageFixture


class Reports(BaseHTTPRequestHandler):
    def log_message(self, *_):
        # Request diagnostics are captured without the ephemeral Authorization header.
        pass

    def reply(self, status, value):
        data = json.dumps(value).encode()
        self.send_response(status)
        self.send_header('Content-Type', 'application/json')
        self.send_header('Content-Length', str(len(data)))
        self.end_headers()
        self.wfile.write(data)

    def authorized(self):
        if self.headers.get('Authorization') != 'Bearer ' + self.server.bearer:
            self.reply(401, {'error': 'unauthorized'})
            return False
        self.server.requests.append((self.command, self.path))
        return True

    def do_POST(self):
        if not self.authorized():
            return
        if self.server.failure is not None:
            return self.reply(self.server.failure, {'error': 'injected_http_error'})
        if self.path != ROUTE:
            return self.reply(404, {})
        body = json.loads(self.rfile.read(int(self.headers['Content-Length'])))
        now = int(self.server.now)
        # Match query_hash's minute scope in both pinned source contracts.
        key = (json.dumps(body, sort_keys=True), now // 60)
        if key not in self.server.queries:
            report_id = 'umr_' + uuid.uuid4().hex
            value = {'schema_version': 1, 'report_id': report_id, 'status': 'pending',
                     'report_complete': False, 'scope': {'admin': True, 'principal': 'fixture'},
                     'query': body, 'as_of': now, 'measurement_cutoff': now - 120,
                     'expires_at': now + 600, 'effective_range': {
                         'from': body['from'], 'to': min(body['to'], now - 120)},
                     'poll_url': ROUTE + '/' + report_id, 'summary': None}
            self.server.queries[key] = report_id
            self.server.reports[report_id] = value
            if self.server.complete_on_admission:
                self.server.finish(report_id)
        value = self.server.reports[self.server.queries[key]]
        self.reply(200 if value['status'] == 'ready' else 202, value)

    def do_GET(self):
        if not self.authorized():
            return
        suffix = urlsplit(self.path).path.removeprefix(ROUTE + '/')
        report_id, _, child = suffix.partition('/')
        value = self.server.reports.get(report_id)
        if value is None:
            return self.reply(404, {})
        if not child:
            return self.reply(200, value)
        if value['status'] != 'ready':
            return self.reply(503, {})
        field = {'sandboxes': 'items', 'series': 'lines', 'owners': 'items'}.get(child)
        if not field:
            return self.reply(404, {})
        self.reply(200, {'report_id': report_id, field: []})


class UsageHttpTests(unittest.TestCase):
    def setUp(self):
        directory = tempfile.TemporaryDirectory()
        self.addCleanup(directory.cleanup)
        self.output = Path(directory.name)
        server = self.server = ThreadingHTTPServer(('127.0.0.1', 0), Reports)
        server.bearer = secrets.token_urlsafe(24)
        server.now = 1_200_000
        server.requests, server.reports, server.queries = [], {}, {}
        server.complete_on_admission = False
        server.failure = None

        def finish(report_id):
            value = server.reports[report_id]
            value.update(status='ready', report_complete=True, summary={'sandbox_count': 0},
                         as_of=int(server.now), measurement_cutoff=int(server.now) - 120,
                         expires_at=int(server.now) + 600)
            value['effective_range']['to'] = min(value['query']['to'], int(server.now) - 120)

        server.finish = finish
        thread = threading.Thread(target=server.serve_forever, daemon=True)
        thread.start()

        def cleanup():
            server.shutdown()
            thread.join(timeout=5)
            server.server_close()
            self.assertFalse(thread.is_alive())

        self.addCleanup(cleanup)
        base = 'http://127.0.0.1:' + str(server.server_port)
        with patch.dict(os.environ, {'MATRIX_STAGE': 'old-write',
                                    'MATRIX_ADMIN_BEARER': server.bearer}, clear=True):
            self.client = Fixture(base, self.output)
        # Pacing itself has separate tests. Advance a simulated clock here so
        # minute-boundary coverage does not occupy a test host for real minutes.
        self.pacer = patch.object(self.client, '_pace_create')
        self.pacer.start()
        self.addCleanup(self.pacer.stop)
        self.fixture = UsageFixture(self.client, clock=lambda: server.now,
                                    sleep=lambda seconds: setattr(server, 'now', server.now + seconds))

    def test_four_stage_http_campaign_keeps_ids_after_minute_rollover(self):
        first = self.fixture.exercise('old-write', [])
        old = json.loads((self.output / 'old-write-usage.private.json').read_text())
        self.assertEqual(old['created']['status'], 'pending')
        self.server.now += 180
        self.server.finish(old['report_id'])
        request_count = len(self.server.requests)
        self.fixture.exercise('new-read-old-write', [])
        self.assertEqual(self.server.requests[request_count:],
                         [('GET', ROUTE + '/' + old['report_id'])])
        self.server.complete_on_admission = True
        second = self.fixture.exercise('new-write', [])
        new = json.loads((self.output / 'new-write-usage.private.json').read_text())
        self.assertNotEqual(new['report_id'], old['report_id'])
        self.assertEqual(new['created']['status'], 'ready')
        snapshot = deepcopy(self.server.reports)
        self.server.now += 180
        request_count = len(self.server.requests)
        self.fixture.exercise('old-read-new-write', [])
        self.assertTrue(all(method == 'GET' for method, _ in self.server.requests[request_count:]))
        self.assertEqual(self.server.reports, snapshot)
        self.assertEqual(first['collections'], second['collections'])
        self.assertEqual(len(self.server.reports), 2)
        log = json.dumps(self.client.requests)
        self.assertNotIn(self.server.bearer, log)
        self.assertEqual({row['status'] for row in self.client.requests}, {200, 202})

    def test_rate_limit_and_service_failure_never_produce_a_writer_receipt(self):
        for status, error in [(429, RateLimited), (503, RuntimeError)]:
            self.server.failure = status
            count = len(self.server.requests)
            with self.subTest(status=status), self.assertRaises(error):
                self.fixture.exercise('old-write', [])
            self.assertEqual(len(self.server.requests) - count, 1)
            self.assertFalse((self.output / 'old-write-usage.private.json').exists())
            self.assertEqual(self.server.reports, {})

    def test_corrupted_ready_snapshot_is_rejected_over_http(self):
        self.server.complete_on_admission = True
        self.fixture.exercise('new-write', [])
        report_id = next(iter(self.server.reports))
        self.server.reports[report_id]['summary']['sandbox_count'] = 99
        with self.assertRaisesRegex(RuntimeError, 'ready report snapshot changed'):
            self.fixture.exercise('old-read-new-write', [])


if __name__ == '__main__':
    unittest.main()
