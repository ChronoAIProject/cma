#!/usr/bin/env python3
"""Exercise a Mongo input budget across both immutable AM image switches.

The first request targets a nonexistent Agent and must reach the domain's 404.
The opposite image's first input must then be refused before the one-minute
budget refills. No Agent or native runtime is created and no Mongo row is injected.
"""
import copy
import json
import os
from pathlib import Path
import re
import time
import uuid

from migration_browser_fixture import COLLECTIONS as AUTH_COLLECTIONS, SHAPES as AUTH_SHAPES
from migration_profile_fixture import Fixture
from migration_release_probe import STAGE_ROLES, declared_reader_artifacts

STAGES = ('old-write', 'new-read-old-write', 'new-write', 'old-read-new-write')
SHAPES = ['shared-input-budget', 'fresh-process-budget-continuity']
WINDOW_SECONDS = 45


def require(condition, message):
    if not condition:
        raise RuntimeError('rate migration: ' + message)


def manifests(objects):
    """Change only the disposable input budget; keep other traffic budgets."""
    result = copy.deepcopy(objects)
    matches = [obj for obj in result if obj.get('kind') == 'ConfigMap'
               and obj.get('metadata', {}).get('name') == 'api-env']
    require(len(matches) == 1, 'exactly one API ConfigMap required')
    values = matches[0]['data']
    require(values.get('CHRONO_AUTH_MODE') == 'provider', 'provider auth required')
    require('CHRONO_SERVER_RATE_INPUT_PER_MINUTE' not in values, 'input budget already set')
    values['CHRONO_SERVER_RATE_INPUT_PER_MINUTE'] = '1'
    return result


class RateFixture:
    def __init__(self, fixture, stage, artifact, reader_uid, plan, clock=time.time):
        require(stage in STAGES, 'unknown stage')
        require(isinstance(reader_uid, str) and bool(reader_uid), 'reader pod UID missing')
        readers = declared_reader_artifacts(plan)
        self.old, self.new = readers['historical'], readers['candidate']
        require(self.old != self.new, 'distinct artifacts required')
        expected = readers[STAGE_ROLES[stage]]
        require(artifact == expected, 'reader artifact differs from plan')
        self.fixture, self.stage, self.artifact = fixture, stage, artifact
        self.reader_uid, self.clock = reader_uid, clock
        self.writer = stage if stage in ('old-write', 'new-write') else (
            'old-write' if stage == 'new-read-old-write' else 'new-write')
        self.role = 'owner' if self.writer == 'old-write' else 'admin'
        require(fixture.browser is not None and bool(fixture.cookies), 'browser custody missing')
        require(self.role != 'admin' or bool(fixture.admin_bearer), 'admin bearer missing')

    def input(self, route, status):
        args = {'key': 'matrix-rate-' + uuid.uuid4().hex, 'status': status}
        if self.role == 'admin':
            args.update(bearer=self.fixture.admin_bearer, use_cookies=False)
        response = self.fixture.request('POST', route, body={'text': 'migration budget probe'}, **args)
        row = self.fixture.requests[-1]
        require(row['status'] == status and isinstance(response, dict), 'wrong HTTP result')
        require(isinstance(row['request_id'], str) and bool(row['request_id']), 'request ID missing')
        require(row['started_at'] <= row['finished_at'], 'request clock moved backwards')
        if status == 404:
            require(response.get('code') in ('resource_not_found', 'agent_not_found'),
                    'request did not reach the Agent domain')
        else:
            require(response.get('code') == 'rate_limited', 'wrong refusal problem')
            delay = self.fixture.last_retry_after
            require(type(delay) is int and 1 <= delay <= 60, 'missing or invalid Retry-After')
        return {'started_at': row['started_at'], 'finished_at': row['finished_at'],
                'request_id': row['request_id'], 'status': status, 'code': response['code'],
                'retry_after': self.fixture.last_retry_after if status == 429 else None}

    def validate_record(self, record):
        expected = self.old if self.writer == 'old-write' else self.new
        require(record.get('schema') == 'cma-migration-rate-writer-v1'
                and record.get('writer') == self.writer and record.get('role') == self.role
                and record.get('artifact') == expected, 'writer provenance differs')
        require(isinstance(record.get('reader_uid'), str) and bool(record['reader_uid']),
                'writer pod UID missing')
        require(re.fullmatch(r'/api/v1/agents/agt_[a-f0-9]{32}/input', record.get('route', '')),
                'writer route differs')
        charged, denied = record['charged'], record['denied']
        require(charged['status'] == 404 and charged['code'] in ('resource_not_found', 'agent_not_found')
                and denied['status'] == 429 and denied['code'] == 'rate_limited',
                'writer results differ')
        require(charged['started_at'] <= charged['finished_at'] <= denied['started_at']
                <= denied['finished_at'] <= self.clock(), 'writer clock order invalid')
        require(all(isinstance(row['request_id'], str) and row['request_id']
                    for row in (charged, denied)), 'writer request IDs missing')
        self.window(charged)
        self.delay(charged, denied)

    def window(self, charged):
        require(0 <= self.clock() - charged['started_at'] < WINDOW_SECONDS,
                'observation missed the unrefilled budget window')

    @staticmethod
    def delay(charged, denied):
        remaining = 60 - (denied['finished_at'] - charged['started_at'])
        delay = denied['retry_after']
        require(type(delay) is int and 1 <= delay <= 60 and abs(delay - remaining) <= 3,
                'Retry-After differs from the retained budget')

    def exercise(self):
        path = self.fixture.output / (self.writer + '-rate.private.json')
        if self.stage == self.writer:
            route = '/api/v1/agents/agt_' + uuid.uuid4().hex + '/input'
            charged = self.input(route, 404)
            denied = self.input(route, 429)
            record = {'schema': 'cma-migration-rate-writer-v1', 'writer': self.writer,
                      'role': self.role, 'artifact': self.artifact, 'reader_uid': self.reader_uid,
                      'route': route, 'charged': charged, 'denied': denied}
            self.validate_record(record)
            path.write_text(json.dumps(record, indent=2) + '\n')
            observation = {'writer': self.writer, 'role': self.role, 'denied': denied}
        else:
            record = json.loads(path.read_text())
            self.validate_record(record)
            require(record['reader_uid'] != self.reader_uid, 'reader process was not replaced')
            # This is the first input for this subject in the new process, so
            # a local limiter cannot explain its refusal. There is no retry.
            denied = self.input(record['route'], 429)
            self.window(record['charged'])
            self.delay(record['charged'], denied)
            observation = {'writer': self.writer, 'role': self.role, 'denied': denied,
                           'writer_uid': record['reader_uid'], 'reader_uid': self.reader_uid,
                           'seconds_since_charge': denied['finished_at'] - record['charged']['started_at']}
        (self.fixture.output / ('rate-' + self.stage + '.json')).write_text(
            json.dumps(observation, indent=2) + '\n')
        return {'collections': ['request_rate_limits'], 'shapes': list(SHAPES),
                'checks': {'domain_route_reached': True, 'budget_refused_before_refill': True,
                           'writer_and_subject_bound': True}, 'samples': 1}


def main():
    os.umask(0o077)
    stage = os.environ['MATRIX_STAGE']
    fixture = Fixture(os.environ['MATRIX_API_URL'], Path.cwd())
    try:
        artifact = {key: os.environ['MATRIX_' + key.upper()] for key in ('source', 'image')}
        plan = json.loads(Path(os.environ['MATRIX_PLAN']).read_text())
        result = RateFixture(fixture, stage, artifact, os.environ['MATRIX_READER_POD_UID'], plan).exercise()
        auth = fixture.browser.finish(stage)
        require(all(value for key, value in auth.items() if key != 'consumed_attempt_rejected'),
                'browser continuity failed')
        (fixture.output / ('browser-' + stage + '.json')).write_text(
            json.dumps(fixture.browser.observations, indent=2) + '\n')
        receipt = {'schema': 'cma-migration-fixture-v1', 'stage': stage, **artifact,
                   **result, 'outcome': 'PASS'}
        receipt['collections'] += AUTH_COLLECTIONS
        receipt['shapes'] += AUTH_SHAPES
        receipt['checks']['browser_continuity'] = True
        (fixture.output / 'fixture-receipt.json').write_text(json.dumps(receipt, indent=2) + '\n')
    finally:
        (fixture.output / ('requests-' + stage + '.private.json')).write_text(
            json.dumps(fixture.requests, indent=2) + '\n')


if __name__ == '__main__':
    main()
