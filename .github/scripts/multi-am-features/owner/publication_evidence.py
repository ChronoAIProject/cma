"""Collect sanitized, Agent-scoped publication diagnostics from both Managers."""
import hashlib
import json
from pathlib import Path
import re

from common import require_feature_fixture


FAILURES = frozenset(('known_aborted_cas', 'owner_authority_refused', 'work_lease_changed', 'unknown_commit'))
STARTUP_STEPS = frozenset(('install_skill', 'wait_for_workspace', 'add_worktree', 'materialize_home',
                           'open_conversation', 'publish_ready', 'publish_connection', 'dispatch_first_message'))
LOG_BOUND_BYTES = 2 * 1024 * 1024
LOG_TARGET = ' agent_scheduler_failure: '
FIELDS = re.compile(r'(?<!\S)(phase|failure|next_step|agent_sha256|cause_class|origin|agent_hash)="?([a-z0-9_]+)"?(?=\s|$)')
PREPARED_MONGO_APP = 'cma-two-am-prepared-fixture'
FEATURES_MONGO_APP = 'cma-two-am-features-fixture'
SCHEMA_RACE = Path(__file__).with_name('prepared_schema_race.js')


def prepared_publication_abort(fixture, arm=None):
    require_feature_fixture(fixture, kind='prepared')
    if type(arm) is not bool:
        raise ValueError('prepared_publication_abort_action_required')
    return publication_abort(fixture, arm, 'update', 'worktrees', PREPARED_MONGO_APP)


def artifact_publication_abort(fixture, arm=None):
    require_feature_fixture(fixture)
    if type(arm) is not bool:
        raise ValueError('artifact_publication_abort_action_required')
    return publication_abort(fixture, arm, 'insert', 'agent_artifacts', FEATURES_MONGO_APP)


def publication_abort(fixture, arm, operation, collection, app):
    command = {'configureFailPoint': 'failCommand', 'mode': 'off'}
    if arm:
        command.update(mode={'times': 1}, data={
            'failCommands': [operation], 'namespace': 'cma_native_am.' + collection,
            'appName': app, 'errorCode': 112,
            'errorLabels': ['TransientTransactionError'],
        })
    return fixture.mongo(f'const result = admin.runCommand({json.dumps(command)});'
                         "if (result.ok !== 1 || result.count == null) throw Error('publication_abort_not_configured');"
                         'print(JSON.stringify({count: Number(result.count)}));')


def publication_events(fixture, manager):
    raw = fixture.client.k(['logs', f'am-{manager}-0', '--since=12m',
                            f'--limit-bytes={LOG_BOUND_BYTES}'], timeout=8)
    if len(raw.encode()) >= LOG_BOUND_BYTES:
        raise RuntimeError('publication_evidence_log_bound')
    for entry in raw.splitlines():
        line = re.sub(r'\x1b\[[0-9;]*m', '', entry)
        if LOG_TARGET in line:
            yield dict(FIELDS.findall(line.split(LOG_TARGET, 1)[1]))


def agent_digest(agentId):
    if not isinstance(agentId, str) or not re.fullmatch(r'[-A-Za-z0-9_]{1,160}', agentId):
        raise ValueError('publication_evidence_lineage_required')
    return hashlib.sha256(agentId.encode()).hexdigest()


def publication_evidence(fixture, agentId=None):
    digest = agent_digest(agentId)
    observations = {}
    for manager in ('a', 'b'):
        counts = {}
        for fields in publication_events(fixture, manager):
            if fields.get('agent_sha256') != digest or fields.get('phase') != 'publication':
                continue
            failure, step = fields.get('failure'), fields.get('next_step')
            if failure not in FAILURES or not step:
                raise RuntimeError('publication_evidence_invalid_diagnostic')
            if step not in STARTUP_STEPS:
                continue
            by_step = counts.setdefault(failure, {})
            by_step[step] = by_step.get(step, 0) + 1
        observations[manager] = counts
    return observations


def prepared_schema_refusals(fixture, agentId=None):
    require_feature_fixture(fixture, kind='prepared')
    digest = agent_digest(agentId)
    return {manager: sum(fields.get('phase') == 'execution' and fields.get('origin') == 'execute'
                         and fields.get('cause_class') == 'stored_data_invalid' and fields.get('agent_hash') == digest
                         for fields in publication_events(fixture, manager)) for manager in ('a', 'b')}


def prepared_schema_barrier(fixture, arm=None):
    require_feature_fixture(fixture, kind='prepared')
    if type(arm) is not bool:
        raise ValueError('prepared_schema_race_action_required')
    command = {'configureFailPoint': 'failCommand', 'mode': 'off'}
    if arm:
        command.update(mode={'times': 20}, data={
            'failCommands': ['update'], 'namespace': 'cma_native_am.worktrees',
            'appName': PREPARED_MONGO_APP, 'blockConnection': True, 'blockTimeMS': 2400,
        })
        profile = {'profile': 2, 'filter': {'appName': PREPARED_MONGO_APP, 'command.find': {
            '$in': ['native_prepared_sandboxes', 'sandboxes', 'workspaces', 'worktrees'],
        }}}
        script = f'if (d.runCommand({json.dumps(profile)}).ok !== 1) throw Error("prepared_schema_profile_arm");\n'
        script += f'if (admin.runCommand({json.dumps(command)}).ok !== 1) throw Error("prepared_schema_barrier_arm");'
    else:
        script = f'try {{if (admin.runCommand({json.dumps(command)}).ok !== 1) throw Error("prepared_schema_barrier_off");}}'
        script += 'finally {if (d.runCommand({profile: 0, filter: "unset"}).ok !== 1) throw Error("prepared_schema_profile_off");}'
    return fixture.mongo(script + 'print(JSON.stringify({configured: true}));')


def prepared_schema_race(fixture, action=None, operationId=None, reservations=None):
    require_feature_fixture(fixture, kind='prepared')
    if (action not in ('inject', 'read', 'restore') or not isinstance(reservations, list)
            or not 1 <= len(reservations) <= 2 or any(not isinstance(value, str)
            or not re.fullmatch(r'[A-Za-z0-9_-]{1,128}', value) for value in [operationId, *reservations])):
        raise ValueError('prepared_schema_race_lineage_required')
    scope = json.dumps({'action': action, 'operationId': operationId, 'reservations': reservations,
                        'app': PREPARED_MONGO_APP})
    return fixture.mongo(f'const scope = {scope};\n' + SCHEMA_RACE.read_text())
