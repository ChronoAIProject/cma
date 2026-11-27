"""Collect sanitized, Agent-scoped publication diagnostics from both Managers."""
import hashlib
import ipaddress
import json
from pathlib import Path
import re

from common import require_feature_fixture


FAILURES = frozenset(('known_aborted_cas', 'owner_authority_refused', 'work_lease_changed', 'unknown_commit'))
STARTUP_STEPS = frozenset(('install_skill', 'wait_for_workspace', 'add_worktree', 'materialize_home',
                           'open_conversation', 'publish_ready', 'publish_connection', 'dispatch_first_message'))
LOG_BOUND_BYTES = 2 * 1024 * 1024
LOG_TARGET = ' agent_scheduler_failure: '
FIELDS = re.compile(r'(?<!\S)(phase|failure|next_step|agent_sha256|cause_class|origin|agent_hash|operation|sandbox_sha256|work_lease_sha256)="?([a-z0-9_]+)"?(?=\s|$)')
RECOVERY_FIELDS = re.compile(r'(?<!\S)(sandbox_id|outcome)="?([-A-Za-z0-9_]+)"?(?=\s|$)')
PREPARED_MONGO_APP = 'cma-two-am-prepared-fixture'
FEATURES_MONGO_APP = 'cma-two-am-features-fixture'
SCHEMA_RACE = Path(__file__).with_name('prepared_schema_race.js')


def artifact_publication_abort(fixture, arm=None, outcome='known'):
    require_feature_fixture(fixture)
    if type(arm) is not bool or outcome not in ('known', 'unknown'):
        raise ValueError('artifact_publication_abort_action_required')
    return publication_abort(fixture, arm, 'insert', 'agent_artifacts', FEATURES_MONGO_APP,
                             unknown=outcome == 'unknown')


def access_proposal_abort(fixture, arm=None):
    require_feature_fixture(fixture)
    if type(arm) is not bool:
        raise ValueError('access_proposal_abort_action_required')
    return publication_abort(fixture, arm, 'insert', 'agent_access_requests', FEATURES_MONGO_APP)


def artifact_publication_race(fixture, action=None, count=None, agentId=None):
    require_feature_fixture(fixture)
    agent_digest(agentId)
    if action not in ('arm', 'wait', 'sample', 'disarm') or (action == 'sample' and count is not None):
        raise ValueError('artifact_publication_race_action_required')
    # Hold the count after its authorization snapshot and before the first
    # lineage write. An insert barrier would already hold the Agent write lock.
    if action != 'sample':
        result = timed_publication_abort(fixture, action, count, 'agent_artifacts', operation='aggregate')
        if action != 'wait':
            return result
        if not result.get('entered'):
            return {'transaction_sha256': None}
    criteria = {'appName': FEATURES_MONGO_APP, 'active': True,
                'command.aggregate': 'agent_artifacts', 'command.autocommit': False}
    identity = fixture.mongo(f'const rows = admin.aggregate([{{$currentOp:{{allUsers:true}}}},'
        f'{{$match:{json.dumps(criteria)}}},{{$limit:8}}]).toArray();'
        f'const target = {json.dumps(agentId)};'
        'const matching = rows.filter(({command}) => command.lsid && command.txnNumber != null && '
        'command.pipeline?.[0]?.$match?.agent_id === target);'
        'if (matching.length > 1) throw Error("artifact_publication_multiple_active_transactions");'
        'print(JSON.stringify(matching.map(({command, opid}) => [command.lsid, command.txnNumber.toString(), opid])));')
    digest = hashlib.sha256(json.dumps(identity[0], sort_keys=True).encode()).hexdigest() if identity else None
    return {'transaction_sha256': digest}


def writer_publication_abort(fixture, arm=None):
    require_feature_fixture(fixture)
    if type(arm) is not bool:
        raise ValueError('writer_publication_abort_action_required')
    return publication_abort(fixture, arm, 'update', 'agents', FEATURES_MONGO_APP)


def runtime_loss_publication_abort(fixture, action=None, count=None, sandboxId=None, manager=None):
    require_feature_fixture(fixture)
    if action not in ('arm', 'wait', 'sample', 'disarm'):
        raise ValueError('runtime_loss_action_required')
    agent_digest(sandboxId)
    if manager not in ('a', 'b'):
        raise ValueError('runtime_loss_manager_required')
    if action == 'arm':
        profile = {'profile': 2, 'filter': {'appName': FEATURES_MONGO_APP, 'command.find': 'sandboxes',
                                            'command.filter._id': sandboxId, 'command.autocommit': False}}
        if fixture.mongo(f'print(JSON.stringify(d.runCommand({json.dumps(profile)}).ok));') != 1:
            raise RuntimeError('runtime_loss_profiler_not_armed')
        return timed_publication_abort(fixture, action, count, 'agents')
    if action == 'disarm':
        try:
            return timed_publication_abort(fixture, action, count, 'agents')
        finally:
            if fixture.mongo('print(JSON.stringify(d.runCommand({profile:0,filter:"unset"}).ok));') != 1:
                raise RuntimeError('runtime_loss_profiler_not_restored')
    if action == 'wait' and not timed_publication_abort(fixture, action, count, 'agents').get('entered'):
        return {'transaction_sha256': None}
    return active_runtime_loss_transaction(fixture, manager, sandboxId)


def active_runtime_loss_transaction(fixture, manager, sandbox_id):
    pod = fixture.client.kjson(['get', 'pod', f'am-{manager}-0', '-o', 'json'])
    client_ip = str(ipaddress.ip_address(pod['status']['podIP']))
    command = {'appName': FEATURES_MONGO_APP, 'active': True, 'command.update': 'agents',
               'command.autocommit': False, 'client': {'$regex': rf'^\[?{re.escape(client_ip)}\]?:[0-9]+$'}}
    read = {'appName': FEATURES_MONGO_APP, 'command.find': 'sandboxes', 'command.filter._id': sandbox_id,
            'command.autocommit': False}
    identity = fixture.mongo(f'const criteria = {json.dumps(read)};'
        f'const rows = admin.aggregate([{{$currentOp:{{allUsers:true}}}},{{$match:{json.dumps(command)}}},{{$limit:8}}]).toArray();'
        'const matches = rows.filter(row => row.command.lsid && row.command.txnNumber != null && '
        'd.system.profile.findOne({...criteria, "command.lsid":row.command.lsid, "command.txnNumber":row.command.txnNumber}));'
        'if (matches.length > 1) throw Error("runtime_loss_multiple_active_transactions");'
        'print(JSON.stringify(matches.map(row => [row.command.lsid, row.command.txnNumber.toString(), row.opid])));')
    digest = hashlib.sha256(json.dumps(identity[0], sort_keys=True).encode()).hexdigest() if identity else None
    return {'transaction_sha256': digest}


def share_redemption_publication_abort(fixture, action=None, count=None):
    require_feature_fixture(fixture)
    # A link redemption fences the recipient first. Revocation does not
    # write this collection and can commit while the recipient is waiting.
    return timed_publication_abort(fixture, action, count, 'sandbox_admissions')


def timed_publication_abort(fixture, action, count, collection, *, operation='update'):
    if action in ('arm', 'disarm') and count is None:
        return publication_abort(fixture, action == 'arm', operation, collection, FEATURES_MONGO_APP,
                                 block_ms=12000)
    if action != 'wait' or type(count) is not int or not 0 <= count < 2**53 - 1:
        raise ValueError('timed_publication_abort_action_required')
    command = {'waitForFailPoint': 'failCommand', 'timesEntered': count + 1, 'maxTimeMS': 4000}
    return fixture.mongo('let entered = false;'
                         f'try {{ entered = admin.runCommand({json.dumps(command)}).ok === 1; }}'
                         'catch (error) { if (error.code !== 50) throw error; }'
                         'print(JSON.stringify({entered}));')


def publication_abort(fixture, arm, operation, collection, app, *, block_ms=None, unknown=False):
    command = {'configureFailPoint': 'failCommand', 'mode': 'off'}
    if arm:
        command.update(mode={'times': 1}, data={
            'failCommands': [operation], 'namespace': 'cma_native_am.' + collection,
            'appName': app, 'errorCode': 112,
            'errorLabels': ['TransientTransactionError'],
        })
        if block_ms is not None:
            command['data'].update(blockConnection=True, blockTimeMS=block_ms)
        if unknown:
            command['data']['errorLabels'].append('UnknownTransactionCommitResult')
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


def writer_publication_evidence(fixture, agentId=None, operation=None):
    require_feature_fixture(fixture)
    if operation not in ('rename', 'skill_install', 'settings'):
        raise ValueError('writer_publication_operation_required')
    return writer_abort_counts(fixture, 'agent_sha256', agent_digest(agentId), operation)


def runtime_loss_evidence(fixture, sandboxId=None):
    require_feature_fixture(fixture)
    digest = agent_digest(sandboxId)
    observations = {}
    for manager in ('a', 'b'):
        events = manager_log_lines(fixture, manager, '6m')
        aborts = scheduled = 0
        for line in events:
            if is_writer_abort(line, 'sandbox_sha256', digest, 'runtime_loss'):
                aborts += 1
            if ('cma::sandbox::reconciliation::loss_observation: runtime loss reconciliation outcome ' in line
                    and dict(RECOVERY_FIELDS.findall(line)) == {
                        'sandbox_id': sandboxId, 'outcome': 'recovery_scheduled'}):
                scheduled += 1
        observations[manager] = {'known_aborts': aborts, 'recovery_scheduled': scheduled}
    return observations


def manager_log_lines(fixture, manager, since):
    raw = fixture.client.k(['logs', f'am-{manager}-0', f'--since={since}',
                            f'--limit-bytes={LOG_BOUND_BYTES}'], timeout=8)
    if len(raw.encode()) >= LOG_BOUND_BYTES:
        raise RuntimeError('writer_publication_evidence_log_bound')
    return (re.sub(r'\x1b\[[0-9;]*m', '', line) for line in raw.splitlines())


def is_writer_abort(line, identity_field, digest, operation):
    if 'cma::agent_manager::writer_publication:' not in line:
        return False
    fields = dict(FIELDS.findall(line))
    return (fields.get(identity_field) == digest and fields.get('operation') == operation
            and fields.get('failure') == 'known_aborted_cas')


def writer_abort_counts(fixture, identity_field, digest, operation):
    return {manager: sum(is_writer_abort(line, identity_field, digest, operation)
                         for line in manager_log_lines(fixture, manager, '2m'))
            for manager in ('a', 'b')}


def publication_evidence(fixture, agentId=None):
    digest = agent_digest(agentId)
    observations = {}
    for manager in ('a', 'b'):
        counts = {}
        for fields in publication_events(fixture, manager):
            if fields.get('agent_sha256') != digest:
                continue
            if fields.get('phase') == 'prepared_commit':
                work_lease = fields.get('work_lease_sha256', '')
                if not re.fullmatch(r'[a-f0-9]{64}', work_lease):
                    raise RuntimeError('prepared_publication_invalid_work_lease_digest')
                counts.setdefault('prepared_commit_work_lease_sha256', []).append(work_lease)
                continue
            if fields.get('phase') != 'publication':
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


def prepared_schema_refusals(fixture, agentId=None, barrier='worktree_schema'):
    require_feature_fixture(fixture, kind='prepared')
    if barrier not in ('worktree_schema', 'worktree_cleanup', 'work_lease'):
        raise ValueError('prepared_parent_barrier_required')
    cause = 'stored_data_invalid' if barrier == 'worktree_schema' else 'store_conflict'
    digest = agent_digest(agentId)
    return {manager: sum(fields.get('phase') == 'execution' and fields.get('origin') == 'execute'
                         and fields.get('cause_class') == cause and fields.get('agent_hash') == digest
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


def prepared_schema_race(fixture, action=None, operationId=None, reservations=None, barrier='worktree_schema'):
    require_feature_fixture(fixture, kind='prepared')
    supported = ('worktree_schema', 'worktree_cleanup', 'work_lease', 'publication_conflict')
    if (action not in ('inject', 'read', 'restore') or barrier not in supported
            or (action == 'read' and barrier == 'publication_conflict')
            or not isinstance(reservations, list)
            or not 1 <= len(reservations) <= 2 or any(not isinstance(value, str)
            or not re.fullmatch(r'[A-Za-z0-9_-]{1,128}', value) for value in [operationId, *reservations])):
        raise ValueError('prepared_schema_race_lineage_required')
    scope = json.dumps({'action': action, 'operationId': operationId, 'reservations': reservations, 'barrier': barrier,
                        'app': PREPARED_MONGO_APP})
    return fixture.mongo(f'const scope = {scope};\n' + SCHEMA_RACE.read_text())
