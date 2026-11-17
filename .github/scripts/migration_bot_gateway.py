#!/usr/bin/env python3
"""Bounded synthetic gateway dependency; never claims real message delivery.

The application images remain the Bot writers/readers. This fixture retains
only gateway responses across application restarts and rejects stale versions
or changed ownership instead of concealing an invalid application request.
"""
from copy import deepcopy
import hashlib
import re
import threading
from urllib.parse import parse_qs, urlsplit
import uuid

PUBLIC = 'https://gateway.matrix.invalid'
CONNECTION = re.compile(r'matrix-(?:old|new)-write-(?:active|disabled|failed|deleted)')


class ContractError(ValueError):
    pass


def require(condition, message):
    if not condition:
        raise ContractError(message)


class Gateway:
    def __init__(self, owner):
        self.owner, self.channels, self.records = owner, {}, {}
        self.lock = threading.RLock()
        self.effects = {'create': 0, 'update': 0, 'delete': 0, 'test': 0}

    def request(self, method, target, body=None):
        with self.lock:
            return self._request(method, target, body)

    def _request(self, method, target, body):
        url = urlsplit(target)
        parts = url.path.strip('/').split('/')
        if parts[:2] == ['v1', 'channel-connections'] and method == 'GET':
            require(len(parts) == 3 and CONNECTION.fullmatch(parts[2]) and not url.query,
                    'connection_scope')
            return 200, {'connection_ref': parts[2], 'status': 'ready', 'label': 'Matrix'}
        if parts[:2] == ['v1', 'channel-records'] and method == 'GET':
            require(len(parts) == 3 and not url.query, 'record_route')
            identifier = self.records.get(parts[2])
            return (200, deepcopy(self.channels[identifier])) if identifier else (404, {})
        require(parts[:2] == ['v1', 'channels'], 'gateway_route')
        if len(parts) == 2 and method == 'POST':
            require(not url.query, 'create_query')
            self.validate_policy(body)
            record = body.get('record_id')
            require(isinstance(record, str) and re.fullmatch(r'[a-f0-9-]{36}', record),
                    'record_identity')
            if record in self.records:
                return 409, {'error': 'existing_record'}
            require(len(self.channels) < 16, 'channel_bound')
            identifier = 'matrix-' + uuid.uuid4().hex
            row = {
                'channel_id': identifier, 'record_id': record, 'version': 1,
                'provider': {'slug': body['provider']['slug'], 'binding_id': identifier,
                             'level': 'l2', 'refused': []},
                'endpoints': {'nyxid_callback_url': PUBLIC + '/v1/callbacks/' + identifier,
                              'custom_event_url': PUBLIC + '/v1/channels/' + identifier + '/events'},
                'definition': {'record_id': record, 'owner': {'kind': 'human', 'subject': self.owner},
                               'profile': deepcopy(body['profile']), 'sources': [],
                               'connection_ref': body['connection_ref'], 'creation': {'ready': True},
                               'secrets': {'agent_key_hash': self.key_hash(body.get('credentials'))}},
            }
            # A real provider refusal must drive the application's durable
            # cleanup path, including its normal delay, without repairing DB rows.
            if body['connection_ref'].endswith('-failed'):
                row['provider']['refused'] = [{'field': 'fixture_admission'}]
            self.channels[identifier] = row
            self.records[record] = identifier
            self.effects['create'] += 1
            return 201, deepcopy(row)
        require(len(parts) in (3, 4), 'channel_route')
        identifier = parts[2]
        row = self.channels.get(identifier)
        if row is None:
            return 404, {}
        if len(parts) == 4:
            require(method == 'POST' and parts[3] == 'test' and not url.query
                    and body is None, 'test_route')
            self.effects['test'] += 1
            return 200, {'accepted_after_ms': 1, 'first_text': 'synthetic gateway response', 'level': 'l2'}
        if method == 'GET':
            require(not url.query, 'read_query')
            return 200, deepcopy(row)
        if method == 'PUT':
            require(not url.query, 'update_query')
            self.validate_policy(body)
            if body.get('expected_version') != row['version']:
                return 409, {'error': 'stale_version'}
            require(body['profile'] == row['definition']['profile']
                    and body['connection_ref'] == row['definition']['connection_ref']
                    and body['provider']['slug'] == row['provider']['slug'], 'changed_binding')
            key_hash = (self.key_hash(body['credentials']) if 'credentials' in body
                        else row['definition']['secrets']['agent_key_hash'])
            row['definition']['secrets']['agent_key_hash'] = key_hash
            row['version'] += 1
            self.effects['update'] += 1
            return 200, deepcopy(row)
        if method == 'DELETE':
            require(set(parse_qs(url.query)) == {'expected_version'}, 'delete_query')
            if parse_qs(url.query)['expected_version'] != [str(row['version'])]:
                return 409, {'error': 'stale_version'}
            del self.records[row['record_id']]
            del self.channels[identifier]
            self.effects['delete'] += 1
            return 204, None
        raise ContractError('gateway_method')

    def validate_policy(self, body):
        require(isinstance(body, dict) and body.get('schema_version') == 1, 'policy_schema')
        require(CONNECTION.fullmatch(body.get('connection_ref', '')) is not None, 'connection_scope')
        require(isinstance(body.get('profile'), dict) and body['profile'], 'profile_required')
        require(body.get('provider') == {'slug': 'cma_codex'}, 'provider_scope')
        require(body.get('sources') == [] and isinstance(body.get('connection_admission'), dict),
                'connection_policy')
        require(body.get('partition') == {'version': 1, 'key': 'conversation_and_sender'}, 'partition')
        require(isinstance(body.get('reply'), dict) and body['reply'].get('sinks') == [{'type': 'source'}],
                'reply_sinks')

    @staticmethod
    def key_hash(credentials):
        require(isinstance(credentials, dict) and set(credentials) == {'agent_key'}, 'credentials_scope')
        value = credentials['agent_key']
        require(isinstance(value, str) and 16 <= len(value) <= 4096, 'credential_bound')
        return 'sha256:' + hashlib.sha256(value.encode()).hexdigest()

    def summary(self):
        with self.lock:
            return {'channels': len(self.channels), 'effects': dict(self.effects)}
