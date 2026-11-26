"""Negative cases for actual membership observations, not a simulated AM."""
from copy import deepcopy
import unittest

from migration_membership_validation import compare, integer, member, timestamp, validate, MEMBERS, OWNERS


def number(value, kind='$numberLong'):
    return {kind: str(value)}


def date(value):
    return {'$date': number(value)}


def row(identity='incarnation-one', ip='10.0.0.2', expiry=16000):
    return {'_id': identity, 'document_schema_version': number(1), 'protocol': number(1),
            'relay_address': f'http://{ip}:8080/', 'draining': False, 'owned_agents': number(0),
            'max_owned_agents': number(4), 'expires_at': date(expiry)}


def snapshot(rows=None, now=1000):
    return {'schema': 'cma-migration-membership-v1', 'database': 'matrix', 'observed_at': date(now),
            'collections': {MEMBERS: [row()] if rows is None else rows, OWNERS: []}}


def pod(uid='pod-one', ip='10.0.0.2'):
    return {'uid': uid, 'name': uid, 'ip': ip, 'image_id': 'sha256:' + '1' * 64,
            'container_id': 'containerd://fixture'}


class MembershipValidationTests(unittest.TestCase):
    def test_exact_schema_widths_and_optional_zero_counters(self):
        value = row()
        value['document_schema_version'] = number(1, '$numberInt')
        value.update(renewal_revision=number(0), effect_revision=number(0))
        self.assertEqual(member(value), value)
        self.assertEqual(integer(number(2**63 - 1)), 2**63 - 1)
        self.assertEqual(timestamp(date(0)), 0)

    def test_wrong_bson_width_null_float_negative_and_overflow_refused(self):
        for value in [None, True, 1, 1.0, {}, {'$numberInt': '1'}, {'$numberLong': 1},
                      {'$numberLong': '01'}, number(-1), number(2**63),
                      {'$numberLong': '1', 'other': '1'}]:
            with self.subTest(value=value), self.assertRaises(ValueError):
                integer(value)
        with self.assertRaises(ValueError):
            integer(number(2**31, '$numberInt'), None)
        for value in ['2026-11-18', {'$date': '2026-11-18'}, {'$date': number(1), 'extra': 1}]:
            with self.assertRaises(ValueError):
                timestamp(value)

    def test_member_field_and_protocol_gaps_refused(self):
        changes = [('_id', ''), ('_id', 'x' * 129), ('_id', 1), ('draining', 'false'),
                   ('document_schema_version', number(2)), ('protocol', number(2)),
                   ('protocol', number(1, '$numberInt')), ('owned_agents', number(1)),
                   ('max_owned_agents', number(0)), ('max_owned_agents', number(256)),
                   ('renewal_revision', number(1)), ('effect_revision', number(1)),
                   ('expires_at', None), ('unknown', 'field')]
        for key, value in changes:
            with self.subTest(key=key, value=value), self.assertRaises(ValueError):
                member({**row(), key: value})
        for key in row():
            value = row(); del value[key]
            with self.assertRaises(ValueError):
                member(value)

    def test_relay_urls_are_bounded_and_unambiguous(self):
        for relay in [None, 'x' * 2049, 'file:///tmp/a', 'http://user:pw@host/',
                      'http://host/path', 'http://host/?q=x', 'http://host/#x',
                      'http://host:bad/', 'http://[broken/', 'http:///']:
            with self.subTest(relay=relay), self.assertRaises(ValueError):
                member({**row(), 'relay_address': relay})

    def test_current_member_and_retained_expired_or_draining_rows(self):
        expired = row('old', '10.0.0.3', 999)
        draining = {**row('draining', '10.0.0.4'), 'draining': True}
        result = validate(snapshot([row(), expired, draining]), pod())
        self.assertEqual(result['member']['_id'], 'incarnation-one')
        self.assertEqual(len(result['members']), 3)

    def test_snapshot_identity_counts_and_unexpected_owner_history_refused(self):
        values = [None, {}, {**snapshot(), 'schema': 'wrong'}, {**snapshot(), 'database': 'prod'},
                  {**snapshot(), 'extra': True}, snapshot([]), snapshot([row()] * 17),
                  snapshot([row(), row()]), snapshot([row('other', '10.0.0.3')]),
                  snapshot([{**row(), 'draining': True}]), snapshot([row(expiry=1000)]),
                  snapshot([row(), row('peer', '10.0.0.3')])]
        extra_owner = snapshot(); extra_owner['collections'][OWNERS] = [{'_id': 'agent'}]
        values += [extra_owner, {**snapshot(), 'collections': {}},
                   {**snapshot(), 'collections': {MEMBERS: {}, OWNERS: []}}]
        for value in values:
            with self.subTest(value=value), self.assertRaises(ValueError):
                validate(value, pod())

    def test_exact_pod_identity_and_ipv4_required(self):
        for value in [None, {}, {**pod(), 'uid': ''}, {**pod(), 'ip': 'not-ip'},
                      {**pod(), 'ip': '::1'}, {**pod(), 'ip': '10.0.0.9'}]:
            with self.subTest(value=value), self.assertRaises(ValueError):
                validate(snapshot(), value)

    def test_heartbeat_and_replacement_with_ttl_removal(self):
        before = validate(snapshot(), pod())
        heartbeat = validate(snapshot([row(expiry=19000)], now=4000), pod())
        self.assertFalse(compare(before, heartbeat)['reader_replaced'])
        after = validate(snapshot([row('second', '10.0.0.3', 40000)], now=25000), pod('second', '10.0.0.3'))
        self.assertTrue(compare(before, after)['reader_replaced'])

    def test_identity_reuse_replacement_counter_and_clock_drift_refused(self):
        before = validate(snapshot(), pod())
        for now, current, current_pod in [
            (1000, row(expiry=19000), pod()),
            (4000, row(expiry=16000), pod()),
            (4000, row('different', expiry=19000), pod()),
            (4000, row(expiry=19000), pod('different')),
            (4000, row(expiry=19000), {**pod(), 'container_id': 'changed'}),
        ]:
            with self.subTest(now=now), self.assertRaises(ValueError):
                compare(before, validate(snapshot([current], now), current_pod))

    def test_retained_mutation_early_disappearance_and_revival_refused(self):
        before = validate(snapshot(), pod())
        new = row('new', '10.0.0.3', 20000)
        for old in [None, {**row(), 'draining': True, 'expires_at': date(15000)},
                    {**row(), 'draining': True, 'relay_address': 'http://10.0.0.4:8080/'},
                    {**row(), 'draining': True, 'effect_revision': number(0)}]:
            after = validate(snapshot([new] + ([] if old is None else [old]), 4000), pod('new', '10.0.0.3'))
            with self.assertRaises(ValueError):
                compare(before, after)
        expired = row('expired', '10.0.0.3', 900)
        before = validate(snapshot([row(), expired]), pod())
        revived = {**expired, 'expires_at': date(20000), 'draining': True}
        with self.assertRaisesRegex(ValueError, 'expired member revived'):
            compare(before, validate(snapshot([row(expiry=19000), revived], 4000), pod()))
        drained = {**expired, 'draining': True}
        before = validate(snapshot([row(), drained]), pod())
        with self.assertRaisesRegex(ValueError, 'draining member revived'):
            compare(before, validate(snapshot([row(expiry=19000), expired], 4000), pod()))


if __name__ == '__main__':
    unittest.main()
