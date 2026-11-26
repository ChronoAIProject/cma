#!/usr/bin/env python3
"""Exercise application-written visitor quotas and immutable admission receipts.

The two writers admit two unstarted Launches each. Retained readers replay
those commands and refuse fresh admissions at the stored daily limit. A
separate read-only collector proves nonempty protocol and quota documents;
HTTP refusals alone could be reconstructed from other Launch rows.
"""
from datetime import datetime, timezone
import json
import uuid

from migration_launch_fixture import LaunchFixture, READERS, require
from migration_profile_fixture import main

COLLECTIONS = ['trigger_launches', 'trigger_launch_events',
               'trigger_launch_input_protocols', 'visitor_launch_days']
SHAPES = ['visitor-launch-draft', 'immutable-input-protocol-on',
          'retained-daily-visitor-quota', 'completed-visitor-launch-replay']


def utc_day():
    return int(datetime.now(timezone.utc).timestamp()) // 86400


class VisitorFixture(LaunchFixture):
    def limit(self, stage):
        route = '/api/v1/admin/settings'
        before = self.fixture.admin_request('GET', route)
        desired = 2 if stage in ('old-write', 'new-read-old-write') else 4
        limits = before['settings']['limits']
        require(type(limits.get('visitor_max_launches_per_day')) is int,
                'visitor daily limit missing')
        if stage in READERS:
            require(limits['visitor_max_launches_per_day'] == desired,
                    'retained daily limit changed')
            return desired
        changed_limits = {**limits, 'visitor_max_launches_per_day': desired}
        changed = self.fixture.admin_request('PATCH', route,
            {'expected_revision': before['revision'], 'limits': changed_limits})
        after = self.fixture.admin_request('GET', route)
        require(after == changed and after['revision'] == before['revision'] + 1
                and after['settings'] == {**before['settings'], 'limits': changed_limits},
                'visitor limit changed unrelated settings or failed to persist')
        (self.output / ('visitor-limit-' + stage + '.private.json')).write_text(
            json.dumps({'before': before, 'after': after}, indent=2) + '\n')
        return desired

    def write(self, stage, profiles):
        self.author.prepare_profiles(stage, profiles)
        target = {row['kind'] + '_profile':
                  {'id': row['created']['id'], 'revision': row['revision']['revision']} for row in profiles}
        content = {'presentation': {'title': 'Visitor matrix ' + stage,
                    'description': 'Retained admission only', 'button_label': 'open_in_cma', 'theme': 'auto'},
                   'target': target,
                   'reuse': {'sandbox': 'always_new', 'workspace': 'always_new', 'agent': 'always_new'},
                   'first_message': 'This unstarted Launch must never dispatch.',
                   'consent': {'always_review': True}}
        trigger = self.author.command('POST', '/api/v1/triggers', {'content': content}, 201)
        published = self.author.command('POST', '/api/v1/triggers/' + trigger['response']['id'] + '/publish',
                                        {'draft_version': 1}, 201)
        route = '/api/v1/triggers/public/' + published['response']['public_id'] + '/launches'
        rows = []
        for _ in range(2):
            command = self.author.command('POST', route, {'start_if_ready': False}, 201)
            launch = '/api/v1/launches/' + command['response']['id']
            snapshot = self.snapshot(launch)
            require(snapshot['detail'].get('state') == 'draft', 'visitor Launch did not stay draft')
            rows.append({'route': launch, 'command': command, 'snapshot': snapshot})
        require(len({row['route'] for row in rows}) == 2, 'visitor admissions reused identity')
        return {'writer': stage, 'day': utc_day(), 'launch_route': route, 'launches': rows}

    def verify(self, record, limit):
        require(len(record.get('launches', [])) == 2 and record.get('day') == utc_day(),
                'visitor fixture missing or crossed UTC day')
        self.no_runtime()
        for row in record['launches']:
            require(self.snapshot(row['route']) == row['snapshot'], 'retained visitor Launch changed')
            cmd = row['command']
            require(self.request(cmd['method'], cmd['route'], body=cmd['body'], key=cmd['key'],
                                 status=cmd['status']) == cmd['response'], 'visitor replay changed')
            require(self.snapshot(row['route']) == row['snapshot'], 'visitor replay mutated admission')
        problem = self.request('POST', record['launch_route'], body={'start_if_ready': False},
                               key='matrix-quota-' + uuid.uuid4().hex, status=429)
        require(problem.get('code') == 'resource_quota' and problem.get('resource') == 'visitor_launches'
                and problem.get('limit') == limit and isinstance(problem.get('reset_at'), str),
                'fresh visitor admission did not return the daily quota refusal')
        reset = datetime.fromisoformat(problem['reset_at'].replace('Z', '+00:00'))
        require(reset.tzinfo is not None and int(reset.timestamp()) == (record['day'] + 1) * 86400,
                'quota refusal has wrong reset day')
        require(type(self.fixture.last_retry_after) is int and self.fixture.last_retry_after > 0,
                'quota refusal lacks Retry-After')
        for row in record['launches']:
            require(self.snapshot(row['route']) == row['snapshot'], 'refused admission mutated old Launch')
        self.no_runtime()
        require(utc_day() == record['day'], 'visitor verification crossed UTC day')
        return problem

    def exercise(self, stage, profiles):
        require(stage in ('old-write', 'new-write') or stage in READERS, 'unsupported stage')
        self.enable_launches(stage)
        limit = self.limit(stage)
        writer = READERS.get(stage, stage)
        path = self.output / (writer + '-visitor.private.json')
        initial_day = utc_day()
        if stage == writer:
            if stage == 'new-write':
                previous = json.loads((self.output / 'old-write-visitor.private.json').read_text())
                require(previous['day'] == initial_day, 'campaign crossed UTC quota boundary')
            record = self.write(stage, profiles)
            require(record['day'] == initial_day, 'writer crossed UTC quota boundary')
            path.write_text(json.dumps(record, indent=2) + '\n')
        else:
            record = json.loads(path.read_text())
        require(record.get('writer') == writer, 'visitor writer identity differs')
        problem = self.verify(record, limit)
        (self.output / ('visitor-refusal-' + stage + '.private.json')).write_text(
            json.dumps(problem, indent=2) + '\n')
        return {'collections': COLLECTIONS, 'shapes': SHAPES, 'samples': 2,
                'checks': {'retained_visitor_launch': True, 'daily_limit_refusal': True,
                           'completed_command_replay': True, 'unstarted_no_native_resources': True}}


if __name__ == '__main__':
    main(VisitorFixture)
