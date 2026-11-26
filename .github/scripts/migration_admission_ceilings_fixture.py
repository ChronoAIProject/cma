#!/usr/bin/env python3
"""Exercise operator admission ceilings and stop history across reader switches.

Each writer stage replaces the ceilings and records one stop and resume through
the public administration routes. The opposite reader must return the same
ceilings, revisions and immutable change history; the next writer must extend
that history without rewriting it. Records are written only by the application
under test. The overlay enables the two default-off operator write gates.
"""
import json
from pathlib import Path

from migration_profile_fixture import main

STAGES = ('old-write', 'new-read-old-write', 'new-write', 'old-read-new-write')
READERS = {'new-read-old-write': 'old-write', 'old-read-new-write': 'new-write'}
GATES = ('CHRONO_ADMISSION_CEILINGS_WRITES_ENABLED', 'CHRONO_ADMISSION_CONTROL_WRITES_ENABLED')
COLLECTIONS = ['platform_settings', 'activity']
SHAPES = ['operator-admission-ceilings', 'admission-ceilings-history',
          'operator-admission-stop-history', 'admission-ceilings-activation']
CEILINGS = {
    'old-write': {'registrations': 1001, 'retained_homes': 101, 'active_sessions': 51,
                  'owner_active_sessions': 5,
                  'existing_reserve': {'retained_homes': 3, 'active_sessions': 2}},
    'new-write': {'registrations': 1002, 'retained_homes': 102, 'active_sessions': 52,
                  'owner_active_sessions': 6,
                  'existing_reserve': {'retained_homes': 4, 'active_sessions': None}},
}


def require(condition, message):
    if not condition:
        raise RuntimeError('admission ceilings migration: ' + message)


def manifests(objects):
    selected = {(obj['kind'], obj['metadata']['name']): obj for obj in objects}
    env = selected[('ConfigMap', 'api-env')]['data']
    require(not any(gate in env for gate in GATES), 'overlay requires unconfigured write gates')
    env.update({gate: 'true' for gate in GATES})
    return objects


def observed(view):
    """The durable projection both readers must agree on, without live counts."""
    require(isinstance(view, dict), 'admission view is not an object')
    ceilings = view.get('ceilings')
    require(isinstance(ceilings, dict) and type(ceilings.get('revision')) is int
            and isinstance(ceilings.get('ceilings'), dict), 'ceilings state missing')
    require(type(view.get('revision')) is int, 'admission revision missing')
    history = view.get('history')
    changes = view.get('ceilings_history')
    require(isinstance(history, list) and isinstance(changes, list), 'history missing')
    require(all(isinstance(row, dict) for row in history + changes), 'history row malformed')
    return {'revision': view['revision'], 'operator_stop': view.get('operator_stop'),
            'ceilings_revision': ceilings['revision'], 'ceilings': ceilings['ceilings'],
            'history': history, 'ceilings_history': changes}


class AdmissionCeilingsFixture:
    def __init__(self, fixture):
        self.fixture = fixture
        self.output = Path(fixture.output)

    def read(self):
        return self.fixture.admin_request('GET', '/api/v1/admin/admission')

    def write(self, stage, before):
        view = self.read()
        require(view.get('writes_available') is True
                and view['ceilings'].get('writes_available') is True, 'operator writes are gated off')
        current = observed(view)
        require(current == before, 'state changed before the writer stage')
        updated = self.fixture.admin_request('PUT', '/api/v1/admin/admission/ceilings', {
            'ceilings': CEILINGS[stage], 'reason': 'Migration matrix ' + stage,
            'expected_revision': current['ceilings_revision']})
        require(isinstance(updated, dict) and updated.get('revision') == current['ceilings_revision'] + 1
                and updated.get('ceilings') == CEILINGS[stage], 'ceilings write did not persist')
        stopped = self.fixture.admin_request('POST', '/api/v1/admin/admission/stop', {
            'reason': 'Migration matrix stop ' + stage, 'expected_revision': current['revision']})
        require(isinstance(stopped, dict) and stopped.get('revision') == current['revision'] + 1
                and isinstance(stopped.get('operator_stop'), dict), 'stop did not persist')
        resumed = self.fixture.admin_request('POST', '/api/v1/admin/admission/resume', {
            'reason': 'Migration matrix resume ' + stage, 'expected_revision': stopped['revision']})
        require(isinstance(resumed, dict) and resumed.get('revision') == stopped['revision'] + 1
                and resumed.get('operator_stop') is None, 'resume did not persist')
        after = observed(self.read())
        require(after['ceilings'] == CEILINGS[stage] and after['revision'] == resumed['revision']
                and after['ceilings_revision'] == updated['revision'], 'written state not readable')
        # Immutable history: the earlier writer's rows remain, unchanged, after the new ones.
        require(after['ceilings_history'][1:] == current['ceilings_history'][:len(after['ceilings_history']) - 1]
                and len(after['ceilings_history']) == min(len(current['ceilings_history']) + 1, 20),
                'ceilings history was rewritten')
        require(after['history'][2:] == current['history'][:len(after['history']) - 2]
                and len(after['history']) == min(len(current['history']) + 2, 20),
                'stop history was rewritten')
        return after

    def exercise(self, stage, profiles):
        require(stage in STAGES, 'unknown stage')
        if stage in READERS:
            writer = READERS[stage]
            record = json.loads((self.output / (writer + '-admission-ceilings.private.json')).read_text())
            require(record.get('writer') == writer, 'writer provenance differs')
            require(observed(self.read()) == record['state'], 'reader does not preserve the written state')
        else:
            if stage == 'old-write':
                before = observed(self.read())
                # Activation absent: nothing written yet, so no ceilings revision or history.
                require(before['ceilings_revision'] == 0 and not before['ceilings_history']
                        and before['operator_stop'] is None, 'unexpected prior ceilings state')
            else:
                prior = json.loads((self.output / 'old-write-admission-ceilings.private.json').read_text())
                before = prior['state']
            record = {'writer': stage, 'state': self.write(stage, before)}
            (self.output / (stage + '-admission-ceilings.private.json')).write_text(
                json.dumps(record, indent=2) + '\n')
        return {'collections': list(COLLECTIONS), 'shapes': list(SHAPES), 'samples': 2,
                'checks': {shape: True for shape in SHAPES}}


if __name__ == '__main__':
    main(AdmissionCeilingsFixture)
