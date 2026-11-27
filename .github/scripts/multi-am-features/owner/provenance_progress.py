"""Exercise the shipped provenance backfill with a disposable Manager database."""

from datetime import datetime, timedelta, timezone
import json
from pathlib import Path
import re

from common import require_feature_fixture
from legacy_manifests import pod, resource
from publication_evidence import FEATURES_MONGO_APP, LOG_BOUND_BYTES, agent_digest, timed_publication_abort

DATABASE = 'cma_native_am'
COLLECTION = 'agent_provenance_progress'
ROLE = 'cma_am_provenance_progress'
SCRIPT = Path(__file__).with_suffix('.js')
COMMIT_SCRIPT = SCRIPT.with_name('provenance_progress_commit.js')


def initialize_progress(fixture):
    require_feature_fixture(fixture)
    client = fixture.client
    image = fixture.ready['expected_api_image']

    def run(name, argument, env):
        client.apply([pod(name, image, resource('500m', '256Mi', '50m', '32Mi'),
                          args=[argument], env=env)])

        def completed():
            phase = client.kjson(['get', 'pod', name, '-o', 'json'])['status']['phase']
            if phase == 'Failed':
                raise RuntimeError('provenance_progress_command_failed')
            return phase == 'Succeeded'

        client.wait(completed, 75, 'provenance_progress_command_timeout')
        logged = client.k(['logs', name, '--limit-bytes=16384'])
        return json.loads(logged)

    role = run('am-provenance-role', '--print-provenance-progress-role',
               [{'name': 'CHRONO_MONGODB_DATABASE', 'value': DATABASE}])
    actions = {
        COLLECTION: {'find', 'insert', 'update', 'createCollection', 'listIndexes'},
        '': {'listCollections'},
    }
    if (role.get('createRole') != ROLE or role.get('roles') != []
            or sorted(role) != ['createRole', 'privileges', 'roles']
            or len(role['privileges']) != len(actions)
            or {grant['resource']['collection']: set(grant['actions'])
                for grant in role['privileges']} != actions
            or any(grant['resource'] != {'db': DATABASE, 'collection': grant['resource']['collection']}
                   for grant in role['privileges'])):
        raise RuntimeError('provenance_progress_role_unbound')
    granted = fixture.mongo(f'if (d.runCommand({json.dumps(role)}).ok !== 1) throw Error("provenance_role_create_failed");'
                           f'd.grantRolesToUser("nativeam", [{json.dumps(ROLE)}]);'
                           'print(JSON.stringify(true));')
    if granted is not True:
        raise RuntimeError('provenance_progress_role_not_granted')
    env = [{'name': name, 'valueFrom': {'secretKeyRef': {'name': 'am-environment', 'key': name}}}
           for name in ('CHRONO_MONGODB_URI', 'CHRONO_MONGODB_DATABASE', 'CHRONO_MONGODB_AUTHORITY_MODE',
                        'CHRONO_AGENT_MANAGER_REPLICAS_ENABLED')]
    initialized = run('am-provenance-initialize', '--provenance-backfill-init', env)
    if initialized.get('current', {}).get('phase') != 'ready' or initialized.get('last_sweep') is not None:
        raise RuntimeError('provenance_progress_initial_state_unbound')
    if run_script(fixture, {'action': 'defer'}) != {'deferred': True}:
        raise RuntimeError('provenance_progress_initial_defer_failed')


def run_script(fixture, scope, script=SCRIPT):
    return fixture.mongo(f'const scope = {json.dumps(scope)};\n' + script.read_text())


def profile_progress(fixture, enabled):
    command = ({'profile': 2, 'filter': {'appName': FEATURES_MONGO_APP, '$or': [
        {'command.findAndModify': COLLECTION}, {'command.aggregate': 'agents'}]}} if enabled else
        {'profile': 0, 'filter': 'unset'})
    return fixture.mongo(f'const result = d.runCommand({json.dumps(command)});'
                         'if (result.ok !== 1) throw Error("provenance_progress_profile_failed");'
                         'print(JSON.stringify(true));')


def commit_outcomes(fixture, proof):
    outcomes = []
    for manager in ('a', 'b'):
        raw = fixture.client.k(['logs', f'am-{manager}-0', '--since-time=' + proof['since'],
                                f'--limit-bytes={LOG_BOUND_BYTES}'], timeout=8)
        if len(raw.encode()) >= LOG_BOUND_BYTES:
            raise RuntimeError('provenance_progress_commit_log_bound')
        for line in raw.splitlines():
            plain = re.sub(r'\x1b\[[0-9;]*m', '', line)
            if ' provenance_commit: ' not in plain:
                continue
            match = re.search(r'\bsession_sha256=([a-f0-9]{64})\s+previous_revision=(\d+)\s+'
                              r'outcome="?(unknown|committed|retry|failed)"?(?=\s|$)', plain)
            if match and match[1] == proof['session_sha256'] and int(match[2]) == proof['claim_revision']:
                outcomes.append(match[3])
    return outcomes


def provenance_backfill(fixture, *, action, agentId, sandboxId, operationId, unknown=False, transaction=None):
    require_feature_fixture(fixture)
    for identity in (agentId, sandboxId, operationId):
        agent_digest(identity)
    proof_action = action in ('verify', 'commit')
    if (type(unknown) is not bool or (transaction is not None) != proof_action
            or (proof_action and ((action == 'commit') != unknown or not isinstance(transaction, str)
                                  or not re.fullmatch(r'[a-f0-9]{64}', transaction)))):
        raise ValueError('provenance_progress_mode_required')
    target = agentId, sandboxId, operationId, unknown
    scope = dict(action=action, agentId=agentId, sandboxId=sandboxId, operationId=operationId,
                 app=FEATURES_MONGO_APP, unknown=unknown)
    state = getattr(fixture, 'provenance_original', None)
    if action == 'arm':
        if state is not None:
            raise RuntimeError('provenance_progress_already_armed')
        # Bind the exact original row before arming or changing the target.
        original = run_script(fixture, {**scope, 'action': 'original'})
        state = {'target': target, 'original': original}
        fixture.provenance_original = state
        profile_progress(fixture, True)
        armed = (run_script(fixture, scope, COMMIT_SCRIPT) if unknown else
                 timed_publication_abort(fixture, 'arm', None, 'agents', operation='aggregate'))
        changed = run_script(fixture, {**scope, 'original': original})
        state['previous_receipt'] = changed.pop('previous_receipt', None)
        return {**changed, **armed}
    if action == 'restore' and state is None:
        return {'restored': False}
    if state is None or state['target'] != target:
        raise RuntimeError('provenance_progress_scope_changed')
    if action in ('sample', 'observe', 'verify'):
        observed = run_script(fixture, {**scope, 'transaction': transaction,
                                        'previous_receipt': state['previous_receipt']})
        if unknown and action == 'sample' and observed.get('commit_transaction_sha256'):
            state['sample'] = observed['commit_transaction_sha256'], observed.pop('claim_revision')
        return observed
    if action == 'commit':
        if 'commit' in state:
            raise RuntimeError('provenance_progress_commit_already_armed')
        sample = state.get('sample')
        if sample is None or sample[0] != transaction or type(sample[1]) is not int:
            raise RuntimeError('provenance_progress_commit_claim_unbound')
        since = (datetime.now(timezone.utc) - timedelta(seconds=2)).strftime('%Y-%m-%dT%H:%M:%SZ')
        proof = run_script(fixture, {**scope, 'transaction': transaction}, COMMIT_SCRIPT)
        state['commit'] = {**proof, 'since': since, 'transaction': transaction, 'claim_revision': sample[1]}
        return {'commit_armed': True}
    if unknown and action in ('settle', 'observe_commit'):
        proof = state.get('commit')
        if proof is None or (action == 'settle') == ('affected' in proof):
            raise RuntimeError('provenance_progress_commit_proof_unavailable')
        outcomes = commit_outcomes(fixture, proof)
        if not outcomes and action == 'settle':
            return {'worker_unknown': False}
        if outcomes != ['unknown']:
            raise RuntimeError('provenance_progress_commit_worker_outcome_unbound')
        receipt = run_script(fixture, {**proof, **scope, 'transaction': proof['transaction']}, COMMIT_SCRIPT)
        proof['affected'] = receipt['commit_acknowledgements_lost']
        return {**receipt, 'worker_unknown': True}
    if action == 'restore':
        try:
            disarmed = (run_script(fixture, scope, COMMIT_SCRIPT) if unknown else
                        timed_publication_abort(fixture, 'disarm', None, 'agents', operation='aggregate'))
        finally:
            profile_progress(fixture, False)
        restored = run_script(fixture, {**scope, 'original': state['original']})
        fixture.provenance_original = None
        return {**restored, **disarmed}
    raise ValueError('provenance_progress_action_required')
