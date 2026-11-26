#!/usr/bin/env python3
"""Retain unstarted Launches, events and all public handoff states across images.

These are application-written Mongo records. The synthetic provider changes
after a terminal handoff is saved, so provider reconstruction cannot satisfy
the retained-reader check. Started/native Launches require a separate campaign.
"""
from copy import deepcopy
import json
import uuid

from migration_launch_provider import SERVICE
from migration_profile_fixture import Fixture, main
from migration_trigger_fixture import TriggerFixture

COLLECTIONS = ['trigger_launches', 'trigger_launch_events', 'service_connection_handoffs']
SHAPES = ['unstarted-launch-review', 'cancelled-launch', 'ordered-launch-events',
          'pending-service-handoff', 'satisfied-service-handoff', 'expired-service-handoff',
          'cancelled-service-handoff', 'failed-service-handoff', 'retained-handoff-intent-replay']
READERS = {'new-read-old-write': 'old-write', 'old-read-new-write': 'new-write'}
HANDOFFS = {'pending': 'pending', 'satisfied': 'completed', 'expired': 'expired',
            'cancelled': 'cancelled', 'failed': 'refused'}


def require(condition, message):
    if not condition:
        raise RuntimeError('launch migration: ' + message)


class LaunchProfileFixture(Fixture):
    def create(self, kind, body):
        if kind == 'sandbox':
            body = deepcopy(body)
            body['content']['nyxid_services'] = [SERVICE]
        return super().create(kind, body)


class LaunchFixture:
    def __init__(self, fixture):
        self.fixture, self.output, self.request = fixture, fixture.output, fixture.request
        self.author = TriggerFixture(fixture)

    def control(self, method='GET', suffix='state'):
        browser = self.fixture.browser
        require(browser is not None and self.fixture.admin_bearer, 'synthetic provider control missing')
        code, _, body = browser.raw(method, browser.identity + '/fixture/launch/' + suffix,
            {'Authorization': 'Bearer ' + self.fixture.admin_bearer})
        require(code == 200, 'provider control failed')
        value = json.loads(body)
        require(isinstance(value, dict) and set(value) == {'creates', 'reads', 'links'}
                and all(type(value[k]) is int and value[k] >= 0 for k in ('creates', 'reads'))
                and isinstance(value['links'], dict), 'provider observation invalid')
        return value

    def no_runtime(self):
        for route in ('/api/v1/sandboxes?limit=200', '/api/v1/agents?limit=200'):
            page = self.request('GET', route)
            require(isinstance(page, dict) and page.get('items') == []
                    and page.get('next') is None, 'unstarted launch allocated native resources')

    def enable_launches(self, stage):
        route = '/api/v1/admin/settings'
        before = self.fixture.admin_request('GET', route)
        require(isinstance(before.get('settings'), dict)
                and type(before.get('revision')) is int, 'operator settings invalid')
        policy = before['settings'].get('triggers')
        require(isinstance(policy, dict) and type(policy.get('enabled')) is bool,
                'Trigger operator policy missing')
        if policy['enabled']:
            return
        require(stage == 'old-write', 'retained Trigger enablement disappeared')
        wanted = {**policy, 'enabled': True}
        changed = self.fixture.admin_request('PATCH', route,
            {'expected_revision': before['revision'], 'triggers': wanted})
        after = self.fixture.admin_request('GET', route)
        require(after == changed and after['revision'] == before['revision'] + 1
                and after['settings'] == {**before['settings'], 'triggers': wanted},
                'local Trigger enablement did not persist exactly')
        (self.output / 'local-trigger-enablement.private.json').write_text(
            json.dumps({'before': before, 'after': after}, indent=2) + '\n')

    def snapshot(self, route):
        detail = self.request('GET', route)
        require(detail.get('state') in ('draft', 'cancelled')
                and detail.get('started_at') is None and detail.get('ready_at') is None,
                'launch unexpectedly started')
        head = detail.get('event_head')
        require(type(head) is int and 0 < head <= 200, 'event head outside finite fixture')
        events = self.request('GET', route + '/events?after=0&limit=200')
        require(events.get('launch_id') == detail['id']
                and events.get('next_cursor') == head and isinstance(events.get('events'), list)
                and len(events['events']) == head, 'incomplete launch event history')
        require(all(isinstance(row, dict) and row.get('launch_id') == detail['id']
                    and row.get('seq') == seq for seq, row in enumerate(events['events'], 1)),
                'launch event order or identity differs')
        return {'detail': detail, 'events': events,
                'requirements': self.request('GET', route + '/requirements')}

    def write(self, stage, profiles):
        self.author.prepare_profiles(stage, profiles)
        target = {row['kind'] + '_profile':
                  {'id': row['created']['id'], 'revision': row['revision']['revision']} for row in profiles}
        content = {'presentation': {'title': 'Matrix launch ' + stage, 'description': 'Retained pre-start state',
                    'button_label': 'open_in_cma', 'theme': 'auto'}, 'target': target,
                   'reuse': {'sandbox': 'always_new', 'workspace': 'always_new', 'agent': 'always_new'},
                   'first_message': 'Never dispatch this unstarted fixture.', 'consent': {'always_review': True}}
        created = self.author.command('POST', '/api/v1/triggers', {'content': content}, 201)
        route = '/api/v1/triggers/' + created['response']['id']
        published = self.author.command('POST', route + '/publish', {'draft_version': 1}, 201)
        launch_route = '/api/v1/triggers/public/' + published['response']['public_id'] + '/launches'
        launches = []
        for cancelled in (False, True):
            command = self.author.command('POST', launch_route, {'start_if_ready': False}, 201)
            value = command['response']
            require(value.get('state') == 'draft', 'review launch was not retained as draft')
            route = '/api/v1/launches/' + value['id']
            commands = [command]
            if cancelled:
                commands.append(self.author.command('POST', route + '/cancel'))
            launches.append({'route': route, 'commands': commands, 'snapshot': self.snapshot(route)})
        launch_id = launches[0]['snapshot']['detail']['id']
        services = launches[0]['snapshot']['detail']['requirements']['services']
        require(any(row.get('service') == SERVICE and row.get('state') == 'missing' for row in services),
                'unmet synthetic requirement missing')
        handoffs = []
        for state, provider_state in HANDOFFS.items():
            before = self.control()
            command = self.author.command('POST', '/api/v1/service-connections',
                {'service_id': SERVICE, 'launch_id': launch_id, 'intent_id': uuid.uuid4().hex}, 201)
            after = self.control()
            added = set(after['links']) - set(before['links'])
            require(after['creates'] == before['creates'] + 1 and len(added) == 1,
                    'handoff did not produce exactly one provider effect')
            external = added.pop()
            self.control('POST', external + '/' + provider_state)
            route = '/api/v1/service-connections/' + command['response']['handoff_id']
            observed = self.request('GET', route)
            require(observed.get('state') == state and observed.get('launch_id') == launch_id
                    and observed.get('service_id') == SERVICE, 'handoff state or binding differs')
            handoffs.append({'state': state, 'external': external, 'command': command,
                             'route': route, 'observed': observed})
            self.control('POST', external + '/pending')
        return {'writer': stage, 'launches': launches, 'handoffs': handoffs}

    def verify(self, record):
        require(len(record.get('launches', [])) == 2 and len(record.get('handoffs', [])) == 5,
                'required launch/handoff variants missing')
        require({row['state'] for row in record['handoffs']} == set(HANDOFFS), 'handoff variants differ')
        self.no_runtime()
        for row in record['launches']:
            require(self.snapshot(row['route']) == row['snapshot'], 'retained launch/event/requirements changed')
            for cmd in row['commands']:
                require(self.request(cmd['method'], cmd['route'], body=cmd['body'], key=cmd['key'],
                        status=cmd['status']) == cmd['response'], 'launch operation replay changed')
            require(self.snapshot(row['route']) == row['snapshot'], 'launch replay mutated retained state')
        evidence = []
        for row in record['handoffs']:
            before = self.control('POST', row['external'] + '/pending')
            observed = self.request('GET', row['route'])
            require(observed == row['observed'], 'retained handoff changed')
            cmd = row['command']
            require(self.request(cmd['method'], cmd['route'], body=cmd['body'], key=cmd['key'],
                    status=cmd['status']) == cmd['response'], 'handoff operation replay changed')
            # A fresh operation key must still find the durable intent index.
            require(self.request(cmd['method'], cmd['route'], body=cmd['body'],
                    key='matrix-intent-' + uuid.uuid4().hex, status=201) == cmd['response'],
                    'retained intent replay changed')
            after = self.control()
            require(after['creates'] == before['creates'] and after['links'] == before['links'],
                    'retained replay repeated a provider effect')
            require(after['reads'] - before['reads'] == (1 if row['state'] == 'pending' else 0),
                    'terminal handoff depended on provider reconstruction')
            evidence.append({'route': row['route'], 'state': row['state'],
                             'before': before, 'after': after, 'observed': observed})
        self.no_runtime()
        return evidence

    def exercise(self, stage, profiles):
        require(stage in ('old-write', 'new-write') or stage in READERS, 'unsupported stage')
        self.enable_launches(stage)
        writer = READERS.get(stage, stage)
        path = self.output / (writer + '-launches.private.json')
        if stage == writer:
            record = self.write(stage, profiles)
            path.write_text(json.dumps(record, indent=2) + '\n')
        else:
            record = json.loads(path.read_text())
            require(record.get('writer') == writer, 'writer provenance differs')
        evidence = self.verify(record)
        (self.output / ('launch-verification-' + stage + '.private.json')).write_text(
            json.dumps(evidence, indent=2) + '\n')
        return {'collections': COLLECTIONS, 'shapes': SHAPES, 'samples': 7,
                'checks': {shape: True for shape in SHAPES}}


if __name__ == '__main__':
    main(LaunchFixture, fixture_type=LaunchProfileFixture)
