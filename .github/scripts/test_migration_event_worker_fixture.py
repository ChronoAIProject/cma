#!/usr/bin/env python3
"""Unit regressions for Event Worker reader/writer fixture checks."""
import json
from pathlib import Path
from types import SimpleNamespace
import tempfile
import unittest

from migration_event_worker_fixture import EventWorkerFixture


def profiles():
    return [{'kind': kind, 'created': {'id': kind + '-id'},
             'revision': {'revision': 1}}
            for kind in ('agent', 'workspace', 'sandbox')]


def response(stage='old-write'):
    source = {
        'name': 'Matrix Event Worker',
        'description': 'retained profile compatibility fixture',
        'agent_profile': {'id': 'agent-id', 'revision': 1},
        'workspace_profile': {'id': 'workspace-id', 'revision': 1},
        'sandbox_profile': {'id': 'sandbox-id', 'revision': 1},
    }
    def view(revision, retired, description=None):
        return {'id': 'ewp_' + 'a' * 32, 'owner': 'owner', 'revision': revision,
                'source': {**source, 'description': description or source['description']},
                'profile': {'metadata': {}}, 'digest': 'sha256:' + 'b' * 64,
                'created_at': '2026-01-01T00:00:00Z', 'updated_at': '2026-01-01T00:00:00Z',
                'retired_at': '2026-01-01T00:00:01Z' if retired else None}
    return source, view


class EventWorkerFixtureTests(unittest.TestCase):
    def test_writer_and_reader_directions_preserve_revisions(self):
        source, view = response()
        rows = {1: view(1, False), 2: view(2, False, 'updated retained description'),
                3: view(3, True, 'updated retained description')}
        calls = []
        pending_created = [False]
        seen_keys = set()
        def request(method, route, body=None, key=None, status=200):
            calls.append((method, route))
            if method == 'POST' and route.endswith('event-worker-profiles'):
                if key not in seen_keys:
                    pending_created[0] = True
                    seen_keys.add(key)
                return rows[1]
            if method == 'PUT':
                return rows[2]
            if method == 'DELETE':
                return rows[3]
            if method == 'GET' and route.endswith('event-worker-profiles/ewp_' + 'a' * 32):
                value = rows[1] if pending_created[0] else rows[3]
                pending_created[0] = False
                return value
            if method == 'GET' and '/revisions/' in route:
                return rows[int(route.rsplit('/', 1)[1])]
            if method == 'GET' and '?' in route:
                return {'items': [], 'next_after': None}
            return rows[3]
        with tempfile.TemporaryDirectory() as directory:
            fixture = EventWorkerFixture(SimpleNamespace(request=request, output=Path(directory)))
            for stage in ('old-write', 'new-read-old-write', 'new-write', 'old-read-new-write'):
                result = fixture.exercise(stage, profiles())
                self.assertEqual(result['collections'], ['event_worker_profiles',
                                                          'event_worker_profile_revisions'])
                self.assertEqual(result['samples'], 3)
                self.assertTrue(all(result['checks'].values()))
        self.assertGreaterEqual(len(calls), 20)

    def test_invalid_profile_set_is_rejected(self):
        fixture = EventWorkerFixture(SimpleNamespace(request=None, output=Path('.')))
        with self.assertRaises(RuntimeError):
            fixture.exercise('old-write', profiles()[:2])

    def test_reader_rejects_changed_revision(self):
        source, view = response()
        saved = {'id': 'ewp_' + 'a' * 32, 'path': '/api/v1/event-worker-profiles/' + 'ewp_' + 'a' * 32,
                 'key': 'key', 'source': source, 'created': view(1, False),
                 'changed': view(2, False, 'updated retained description'),
                 'retired': view(3, True, 'updated retained description')}
        def request(method, route, body=None, key=None, status=200):
            if '/revisions/2' in route:
                value = view(2, False, 'wrong')
                return value
            if '/revisions/1' in route:
                return saved['created']
            if '/revisions/3' in route:
                return saved['retired']
            if route.endswith('event-worker-profiles?limit=200'):
                return {'items': [], 'next_after': None}
            if method == 'POST':
                return saved['created']
            return saved['retired']
        with tempfile.TemporaryDirectory() as directory:
            fixture = EventWorkerFixture(SimpleNamespace(request=request, output=Path(directory)))
            with self.assertRaises(RuntimeError):
                fixture._read('old-write', saved)


if __name__ == '__main__':
    unittest.main()
