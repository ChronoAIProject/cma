#!/usr/bin/env python3
"""Exercise retained independent custody through actual Sandbox command replay."""
import json
import os
from urllib.parse import urlsplit
from urllib.request import Request, urlopen

from migration_api_custody_provider import SLOTS, metadata
from migration_browser_fixture import OWNER
from migration_profile_fixture import main
from migration_sandbox_control_fixture import ControlProfileFixture, SandboxControlFixture
from migration_sandbox_fixture import require

COLLECTIONS = ['auth_api_bearers']
SHAPES = ['expiring-independent-api-custody', 'opposite-reader-custody-decryption']


def manifests(objects):
    from migration_api_custody_documents import infrastructure
    from migration_api_custody_provider import manifests as provider
    from migration_sandbox_control_fixture import manifests as controls
    return infrastructure(provider(controls(objects)))


def fetch(fixture, slot):
    require(slot in SLOTS, 'invalid API custody slot')
    base = os.environ.get('MATRIX_IDENTITY_URL', '')
    parsed = urlsplit(base)
    require(parsed.scheme == 'http' and parsed.hostname == '127.0.0.1'
            and not parsed.username and not parsed.password and not parsed.query
            and not parsed.fragment and parsed.path in ('', '/'), 'custody provider must be loopback')
    require(bool(fixture.admin_bearer), 'custody fixture admin credential missing')
    request = Request(base.rstrip('/') + '/fixture/api-custody/' + slot, method='POST', data=b'',
                      headers={'Authorization': 'Bearer ' + fixture.admin_bearer})
    # This response contains a synthetic credential. Never pass it through the
    # application request recorder or include a response body in an exception.
    with urlopen(request, timeout=10) as response:
        require(response.status == 200, 'custody provider refused credential')
        raw = response.read(8193)
    require(len(raw) <= 8192, 'custody provider response exceeds bound')
    value = json.loads(raw)
    require(isinstance(value, dict) and set(value) == {'slot', 'access_token', 'metadata'}
            and value['slot'] == slot, 'custody response identity differs')
    token, observed = value['access_token'], value['metadata']
    require(isinstance(token, str) and 0 < len(token) <= 4096
            and all(32 < ord(c) < 127 for c in token), 'invalid synthetic custody token')
    require(isinstance(observed, dict) and type(observed.get('expires_at')) is int
            and observed == metadata(token, OWNER, observed['expires_at']),
            'custody metadata differs from exact credential')
    return token, {'slot': slot, **observed}


class ApiCustodyFixture(SandboxControlFixture):
    def __init__(self, fixture):
        super().__init__(fixture)
        self.by_sandbox, self.by_command = {}, {}
        self.writer, self.index = None, 0

    def request(self, method, route, body=None, key=None, status=200):
        token = None
        if route.startswith('/api/v1/sandbox-commands/'):
            token = self.by_command.get(route.rsplit('/', 1)[-1])
        elif route.startswith('/api/v1/sandboxes/') and route.endswith('/commands'):
            token = self.by_sandbox.get(route.split('/')[4])
        else:
            return super().request(method, route, body, key, status)
        require(token is not None, 'control request lacks independent credential')
        result = self.fixture.request(method, route, body=body, key=key, status=status,
                                      bearer=token, use_cookies=False)
        if status == 202 and isinstance(result, dict) and 'command_id' in result:
            self.by_command[result['command_id']] = token
        return result

    def write_control(self, identifier):
        slot = f'{self.writer}/{self.index}'
        token, observed = fetch(self.fixture, slot)
        self.index += 1
        self.by_sandbox[identifier] = token
        return {**super().write_control(identifier), 'custody': observed}

    def verify_controls(self, commands):
        require(isinstance(commands, list) and len(commands) == 2,
                'independent custody commands missing')
        slots = []
        for command in commands:
            expected = command.get('custody')
            require(isinstance(expected, dict), 'command custody metadata missing')
            token, observed = fetch(self.fixture, expected.get('slot'))
            require(observed == expected, 'retained custody credential or expiry changed')
            saved = command['receipt']
            self.by_sandbox[saved['sandbox_id']] = token
            self.by_command[saved['command_id']] = token
            slots.append(observed['slot'])
        require(slots == [f'{self.writer}/{index}' for index in range(2)], 'custody writer slots differ')
        return super().verify_controls(commands)

    def exercise(self, stage, profiles):
        self.writer = stage if stage in ('old-write', 'new-write') else (
            'old-write' if stage == 'new-read-old-write' else 'new-write')
        self.index = 0
        try:
            result = super().exercise(stage, profiles)
            return {**result, 'collections': result['collections'] + COLLECTIONS,
                    'shapes': result['shapes'] + SHAPES,
                    'checks': {**result['checks'], **{s: True for s in SHAPES}}}
        finally:
            self.by_sandbox.clear()
            self.by_command.clear()


if __name__ == '__main__':
    main(ApiCustodyFixture, fixture_type=ControlProfileFixture)
