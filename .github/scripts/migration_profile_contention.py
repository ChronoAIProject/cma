"""Exercise profile revision fences and durable update replay through the AM API.

Two independent intents race from the same version. Their completed operation
responses are retained for exact replay by the other reader after an image
switch; the current profile and its published revision must remain untouched.
"""
from concurrent.futures import ThreadPoolExecutor
from copy import deepcopy
from threading import Barrier
import math
import uuid


SHAPES = ['profile-concurrent-version-fence', 'profile-completed-update-replay']


def require(condition, message):
    if not condition:
        raise RuntimeError('profile contention fixture: ' + message)


def intent(kind, before, label):
    require(kind in ('agent', 'workspace', 'sandbox'), 'unsupported profile kind')
    version = before.get('draft_version')
    require(type(version) is int and version > 0 and isinstance(before.get('draft'), dict),
            'invalid retained draft')
    draft = deepcopy(before['draft'])
    if kind == 'agent':
        draft['system_prompt'] = 'Migration concurrent preference ' + label
    elif kind == 'workspace':
        draft['name'] = 'Migration workspace ' + label
    else:
        draft['environment_variables'] = [{'name': 'MATRIX_EDIT', 'value': label}]
    return {'draft_version': version, 'name': 'Migration profile ' + label,
            'description': 'Concurrent migration edit ' + label, 'content': draft}


def race(fixture, kind, path, before):
    commands = [{'key': 'migration-edit-' + uuid.uuid4().hex,
                 'body': intent(kind, before, label)} for label in ('left', 'right')]
    barrier = Barrier(2, timeout=10)

    def execute(command):
        barrier.wait()
        return fixture.request('PUT', path, body=command['body'], key=command['key'],
                               status=(200, 409))

    # Joining both requests is required even if one raises. There is no retry
    # after transport errors, throttling, an unknown result, or a failed fence.
    with ThreadPoolExecutor(max_workers=2) as pool:
        futures = [pool.submit(execute, command) for command in commands]
        responses = [future.result() for future in futures]
    for command, response in zip(commands, responses):
        observed = [row for row in fixture.requests if row['idempotency_key'] == command['key']]
        require(len(observed) == 1, 'each racing intent must have exactly one request')
        require(observed[0]['method'] == 'PUT' and observed[0]['route'] == path
                and observed[0]['response'] == response, 'request observation mismatch')
        timing = {key: observed[0].get(key) for key in ('started_at', 'finished_at')}
        require(all(type(value) in (int, float) and math.isfinite(value)
                    for value in timing.values())
                and timing['finished_at'] > timing['started_at'], 'invalid race timing')
        command.update(status=observed[0]['status'], response=response,
                       request_id=observed[0].get('request_id'), **timing)
    require(max(command['started_at'] for command in commands)
            < min(command['finished_at'] for command in commands),
            'fixture precondition failed: HTTP edit attempts did not overlap')
    require(sorted(command['status'] for command in commands) == [200, 409],
            'race must accept exactly one edit and reject the stale edit')
    winner = next(command for command in commands if command['status'] == 200)
    loser = next(command for command in commands if command['status'] == 409)
    require(isinstance(loser['response'], dict)
            and loser['response'].get('code') == 'profile_version_conflict',
            'rejection is not a profile revision conflict')
    after = fixture.request('GET', path)
    require(after == winner['response'], 'winner response does not match persisted profile')
    require(after.get('id') == before.get('id')
            and after.get('draft_version') == before['draft_version'] + 1,
            'race changed identity or advanced the version more than once')
    for field, request_field in (('name', 'name'), ('description', 'description'),
                                 ('draft', 'content')):
        require(after.get(field) == winner['body'][request_field], 'winner intent was not preserved')
    for field in ('owner', 'visibility', 'created_at', 'latest_revision'):
        require(after.get(field) == before.get(field), 'race changed retained ' + field)
    receipt = {'kind': kind, 'path': path, 'commands': commands}
    verify(fixture, receipt, after)
    return after, receipt


def verify(fixture, receipt, current):
    path = receipt['path']
    commands = receipt['commands']
    require(isinstance(commands, list) and len(commands) == 2
            and sorted(command['status'] for command in commands) == [200, 409]
            and len({command['key'] for command in commands}) == 2,
            'incomplete retained race receipt')
    require(fixture.request('GET', path) == current, 'profile changed before replay')
    for command in commands:
        replay = fixture.request('PUT', path, body=command['body'], key=command['key'],
                                 status=command['status'])
        require(replay == command['response'], 'completed update response changed on replay')
        require(fixture.request('GET', path) == current, 'replay mutated the profile')
    # A fresh stale intent must still be fenced after completed-command replay.
    stale = deepcopy(commands[0]['body'])
    stale['name'] = 'Migration stale overwrite must not be stored'
    refused = fixture.request('PUT', path, body=stale,
                              key='migration-stale-' + uuid.uuid4().hex, status=409)
    require(isinstance(refused, dict) and refused.get('code') == 'profile_version_conflict',
            'fresh stale update lacks the typed revision conflict')
    require(fixture.request('GET', path) == current, 'stale rejection mutated the profile')
