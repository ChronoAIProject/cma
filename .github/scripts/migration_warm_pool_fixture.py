#!/usr/bin/env python3
"""Retain zero-capacity pool plans through the real operator API.

The runtime-read-only manifest prevents pool or runtime creation. This tests
durable targets, original command identity and revision fences, not native pool
readiness, bound Kubernetes UIDs, retirement or positive-capacity operation.
"""
import hashlib
import json
from pathlib import Path
import re
import time
import uuid

from migration_kind_manifests import RUNTIME
from migration_profile_fixture import main
from migration_sandbox_fixture import STAGES

ROUTE = '/api/v1/admin/warm-pools'
ACTOR = 'native-am-fixture-admin'
COLLECTIONS = ['native_warm_pool_targets']
SHAPES = ['zero-capacity-pool-plan', 'pool-config-command-replay', 'pool-revision-fence']
FIELDS = ('id', 'revision', 'image', 'config_sha256', 'cpu_millis', 'memory_mib', 'target_replicas')
ERRORS = (None, 'Native warm pool inventory is unavailable.',
          'Native warm pool reconciliation is unavailable; the recorded target will retry.')


def require(condition, message):
    if not condition:
        raise RuntimeError('warm pool migration: ' + message)


class WarmPoolFixture:
    def __init__(self, fixture, clock=time.monotonic, sleep=time.sleep):
        self.fixture, self.output = fixture, Path(fixture.output)
        self.clock, self.sleep = clock, sleep

    def request(self, method, route, body=None, key=None, status=200):
        require(bool(self.fixture.admin_bearer), 'operator bearer missing')
        return self.fixture.request(method, route, body=body, key=key, status=status,
                                    bearer=self.fixture.admin_bearer, use_cookies=False)

    @staticmethod
    def stable(value):
        # Released readers since 59dbb9d82e also serialize `machine`; these
        # zero-capacity plans and the system default are never machine pools.
        require(isinstance(value, dict) and set(value) - {'machine'} == set(FIELDS) | {
            'ready_replicas', 'observed', 'status', 'resource_name', 'error'}, 'pool view fields differ')
        require(value.get('machine', False) is False, 'pool unexpectedly targets the machine image')
        require(isinstance(value['id'], str) and re.fullmatch(r'wp_[a-f0-9]{64}', value['id']),
                'invalid target identity')
        require(type(value['revision']) is int and value['revision'] in (1, 2), 'invalid revision')
        require(value['image'] == RUNTIME and isinstance(value['config_sha256'], str)
                and re.fullmatch(r'[a-f0-9]{64}', value['config_sha256']), 'pool plan identity differs')
        require(all(type(value[k]) is int and 1 <= value[k] <= 1_048_576
                    for k in ('cpu_millis', 'memory_mib')), 'invalid pool resources')
        require(type(value['target_replicas']) is int and value['target_replicas'] == 0,
                'fixture cannot request runtime capacity')
        require(value['ready_replicas'] is None and value['observed'] is False
                and value['resource_name'] is None and value['status'] in ('pending', 'unavailable')
                and value['error'] in ERRORS, 'unexpected provider state')
        return {key: value[key] for key in FIELDS}

    def inventory(self):
        value = self.request('GET', ROUTE)
        # Released readers since 59dbb9d82e add `machine_image` (string or null).
        require(isinstance(value, dict) and set(value) - {'machine_image'} == {
            'items', 'default_image', 'default_cpu_millis', 'default_memory_mib',
            'default_config_sha256', 'available'}, 'inventory contract differs')
        require(value.get('machine_image') is None or isinstance(value['machine_image'], str),
                'inventory machine image differs')
        require(value['default_image'] == RUNTIME and type(value['available']) is bool,
                'inventory image or availability differs')
        require(all(type(value[k]) is int and value[k] > 0
                    for k in ('default_cpu_millis', 'default_memory_mib'))
                and isinstance(value['default_config_sha256'], str)
                and re.fullmatch(r'[a-f0-9]{64}', value['default_config_sha256']),
                'default plan is missing')
        require(isinstance(value['items'], list) and len(value['items']) <= 3, 'target count exceeded')
        rows = {row['id']: row for row in value['items']}
        require(len(rows) == len(value['items']), 'duplicate pool identity')
        for row in rows.values():
            self.stable(row)
        # Startup installs its own durable target even when desired capacity is
        # zero. Bind the exact system actor/key, not an arbitrary extra row.
        key = 'system-default-v1-' + value['default_config_sha256']
        identifier = 'wp_' + hashlib.sha256(json.dumps(
            ['cma-system', key], separators=(',', ':')).encode()).hexdigest()
        self.default = {'id': identifier, 'revision': 1, 'image': value['default_image'],
            'config_sha256': value['default_config_sha256'],
            'cpu_millis': value['default_cpu_millis'], 'memory_mib': value['default_memory_mib'],
            'target_replicas': 0}
        require(identifier in rows and self.stable(rows[identifier]) == self.default,
                'system default identity or plan differs')
        path = self.output / 'warm-pool-default.private.json'
        if path.exists():
            require(json.loads(path.read_text()) == self.default, 'retained system default changed')
        return rows

    def verify(self, record):
        current, body, key = record['current'], record['body'], record['key']
        expected_id = 'wp_' + hashlib.sha256(json.dumps(
            [ACTOR, key], separators=(',', ':')).encode()).hexdigest()
        require(current['id'] == expected_id and current['revision'] == 2
                and all(current[k] == v for k, v in body.items()), 'saved command identity differs')
        inventory = self.inventory()
        require(current['id'] in inventory, 'retained target missing')
        require(self.stable(inventory[current['id']]) == current, 'retained target changed')
        replay = self.request('POST', ROUTE, body, key, 202)
        require(self.stable(replay) == current, 'command replay changed target')
        for method, route, changed, command_key in [
            ('POST', ROUTE, {**body, 'cpu_millis': body['cpu_millis'] + 1}, key),
            ('POST', ROUTE, body, 'matrix-pool-duplicate-' + uuid.uuid4().hex),
            ('PUT', ROUTE + '/' + current['id'], {'expected_revision': 1, 'target_replicas': 0}, None),
        ]:
            refused = self.request(method, route, changed, command_key, 409)
            require(refused.get('code') == 'operator_conflict', 'wrong command refusal')
        after = self.inventory()
        require(self.stable(after[current['id']]) == current, 'refused write changed target')
        # A second ID for this plan would make retries create duplicate pools.
        require(sum(row['config_sha256'] == current['config_sha256'] for row in after.values()) == 1,
                'command replay duplicated plan')
        expected = {current['id']: current, self.default['id']: self.default}
        for writer in ('old-write', 'new-write'):
            saved = self.output / (writer + '-warm-pool.private.json')
            if saved.exists():
                prior = json.loads(saved.read_text())['current']
                expected[prior['id']] = prior
        require({identifier: self.stable(row) for identifier, row in after.items()} == expected,
                'replay changed retained target inventory')

    def wait_reconciliation(self, identifier):
        deadline = self.clock() + 40
        for _ in range(21):
            row = self.inventory().get(identifier)
            require(row is not None, 'target disappeared during reconciliation')
            # The denied provider write must finish and durably release its
            # lease before the single resize command. No mutation is retried.
            if row['error'] == ERRORS[2]:
                return
            if self.clock() >= deadline:
                break
            self.sleep(2)
        raise RuntimeError('warm pool migration: denied provider reconciliation did not finish')

    def exercise(self, stage, _profiles):
        require(stage in STAGES, 'unknown stage')
        self.inventory()
        default_path = self.output / 'warm-pool-default.private.json'
        if stage == 'old-write':
            require(not default_path.exists(), 'writer already invoked')
            default_path.write_text(json.dumps(self.default, indent=2) + '\n')
        else:
            require(default_path.exists(), 'original system default missing')
        writer = stage if stage in ('old-write', 'new-write') else (
            'old-write' if stage == 'new-read-old-write' else 'new-write')
        path = self.output / (writer + '-warm-pool.private.json')
        if stage == writer:
            body = {'cpu_millis': 1000 if stage == 'old-write' else 2000,
                    'memory_mib': 1024 if stage == 'old-write' else 2048, 'target_replicas': 0}
            key = 'matrix-pool-' + uuid.uuid4().hex
            created = self.stable(self.request('POST', ROUTE, body, key, 202))
            expected_id = 'wp_' + hashlib.sha256(json.dumps(
                [ACTOR, key], separators=(',', ':')).encode()).hexdigest()
            require(created['id'] == expected_id and created['revision'] == 1
                    and all(created[k] == v for k, v in body.items()), 'original command identity differs')
            self.wait_reconciliation(created['id'])
            current = self.stable(self.request('PUT', ROUTE + '/' + created['id'],
                {'expected_revision': 1, 'target_replicas': 0}, status=202))
            require(current == {**created, 'revision': 2}, 'resize changed original plan')
            record = {'writer': writer, 'body': body, 'key': key, 'current': current}
            self.verify(record)
            path.write_text(json.dumps(record, indent=2) + '\n')
        else:
            record = json.loads(path.read_text())
            require(record.get('writer') == writer and record.get('current', {}).get('revision') == 2,
                    'writer provenance differs')
            self.verify(record)
        return {'collections': list(COLLECTIONS), 'shapes': list(SHAPES), 'samples': 1,
                'checks': dict.fromkeys(SHAPES, True)}


if __name__ == '__main__':
    main(WarmPoolFixture)
