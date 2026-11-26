"""Validate declared membership shapes without inferring Agent-owner safety."""
import ipaddress
import re
from urllib.parse import urlsplit

MEMBERS = 'cma_agent_manager_members'
OWNERS = 'cma_agent_manager_owners'


def require(condition, message):
    if not condition:
        raise ValueError('membership migration: ' + message)


def integer(value, width=64):
    keys = ('$numberInt', '$numberLong') if width is None else ('$numberLong',)
    require(isinstance(value, dict) and len(value) == 1 and next(iter(value)) in keys,
            'canonical BSON integer width differs')
    key, text = next(iter(value.items()))
    require(isinstance(text, str) and re.fullmatch(r'0|[1-9][0-9]{0,18}', text),
            'invalid unsigned BSON integer')
    number = int(text)
    require(number <= (2**31 - 1 if key == '$numberInt' else 2**63 - 1),
            'BSON integer exceeds range')
    return number


def timestamp(value):
    require(isinstance(value, dict) and set(value) == {'$date'},
            'canonical BSON DateTime missing')
    return integer(value['$date'])


def member(row):
    fields = {'_id', 'document_schema_version', 'protocol', 'relay_address',
              'draining', 'owned_agents', 'max_owned_agents', 'expires_at'}
    require(isinstance(row, dict) and fields <= set(row)
            and set(row) <= fields | {'renewal_revision', 'effect_revision'},
            'membership fields differ from declared shape')
    require(isinstance(row['_id'], str) and 0 < len(row['_id'].encode()) <= 128,
            'invalid process incarnation')
    require(integer(row['document_schema_version'], None) == 1
            and integer(row['protocol']) == 1, 'unsupported schema or protocol')
    relay = row['relay_address']
    require(isinstance(relay, str) and len(relay.encode()) <= 2048, 'invalid relay')
    try:
        parsed = urlsplit(relay)
        host = parsed.hostname
        parsed.port
    except ValueError as error:
        raise ValueError('membership migration: invalid relay URL') from error
    require(parsed.scheme in ('http', 'https') and host and parsed.path == '/'
            and not parsed.username and not parsed.password and not parsed.query
            and not parsed.fragment, 'invalid relay URL')
    require(type(row['draining']) is bool, 'draining is not boolean')
    require(integer(row['owned_agents']) == 0, 'unexpected owned Agents')
    require(integer(row['max_owned_agents']) == 4, 'fixture capacity differs')
    for field in ('renewal_revision', 'effect_revision'):
        if field in row:
            require(integer(row[field]) == 0, 'unexpected ownership or effect activity')
    timestamp(row['expires_at'])
    return row


def validate(value, pod):
    require(isinstance(value, dict) and set(value) == {
        'schema', 'database', 'observed_at', 'collections'}, 'snapshot fields differ')
    require(value['schema'] == 'cma-migration-membership-v1'
            and value['database'] == 'matrix', 'snapshot identity differs')
    now = timestamp(value['observed_at'])
    collections = value['collections']
    require(isinstance(collections, dict) and set(collections) == {MEMBERS, OWNERS},
            'collection set differs')
    require(collections[OWNERS] == [], 'ownership history is outside this fixture')
    rows = collections[MEMBERS]
    require(isinstance(rows, list) and 1 <= len(rows) <= 16, 'membership row count invalid')
    decoded = [member(row) for row in rows]
    require(len({row['_id'] for row in decoded}) == len(decoded), 'duplicate member identity')
    require(isinstance(pod, dict) and all(isinstance(pod.get(k), str) and pod[k]
            for k in ('uid', 'name', 'ip', 'image_id')), 'serving Pod binding missing')
    try:
        address = ipaddress.ip_address(pod['ip'])
    except ValueError as error:
        raise ValueError('membership migration: invalid Pod IP') from error
    require(address.version == 4, 'fixture requires kind IPv4')
    relay = f'http://{address}:8080/'
    current = [row for row in decoded if row['relay_address'] == relay
               and not row['draining'] and timestamp(row['expires_at']) > now]
    require(len(current) == 1, 'exact live serving member missing or ambiguous')
    require(all(row is current[0] or row['draining'] or timestamp(row['expires_at']) <= now
                for row in decoded), 'unexpected live peer member')
    return {'now': now, 'member': current[0], 'members': {r['_id']: r for r in decoded},
            'pod': pod}


def compare(before, after):
    require(after['now'] > before['now'], 'server observation did not advance')
    old, new = before['member'], after['member']
    same_pod = before['pod']['uid'] == after['pod']['uid']
    require((old['_id'] == new['_id']) == same_pod,
            'process incarnation does not match reader replacement')
    for identity, row in before['members'].items():
        current = after['members'].get(identity)
        if current is None:
            require(timestamp(row['expires_at']) <= after['now'], 'member disappeared before expiry')
            continue
        stable = set(row) - {'expires_at', 'draining'}
        require(set(current) == set(row) and all(current[k] == row[k] for k in stable),
                'retained membership fields changed')
        require(not row['draining'] or current['draining'], 'draining member revived')
        previous_expiry, expiry = timestamp(row['expires_at']), timestamp(current['expires_at'])
        require(expiry >= previous_expiry, 'membership expiry rewound')
        if previous_expiry <= before['now']:
            require(expiry == previous_expiry, 'expired member revived')
    if same_pod:
        require(before['pod'] == after['pod'], 'serving container identity changed')
        require(timestamp(new['expires_at']) > timestamp(old['expires_at']),
                'heartbeat expiry did not advance')
    return {'reader_replaced': not same_pod, 'retained_members_checked': len(before['members'])}
