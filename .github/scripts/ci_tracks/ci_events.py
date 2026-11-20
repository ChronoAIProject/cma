"""Validate the CI event boundary independently of CD comparison policy."""

import re

from git_comparison import check_sha


def repository_name(value):
    if not isinstance(value, str) or not re.fullmatch(r'[A-Za-z0-9_.-]+/[A-Za-z0-9_.-]+', value):
        raise ValueError('invalid repository identity')
    return value


def full_ci_required(event, event_name, repository):
    repository_name(repository)
    if event['repository']['full_name'] != repository:
        raise ValueError('event repository differs')
    if event_name == 'push':
        if event['ref'] != 'refs/heads/develop' or event['deleted'] is not False:
            raise ValueError('CI push must update develop')
        check_sha(event['before'])
        if check_sha(event['after']) == '0' * 40:
            raise ValueError('CI push requires a live commit')
        return True
    if event_name != 'pull_request':
        raise ValueError('unsupported CI event')
    if type(event['number']) is not int or event['number'] <= 0:
        raise ValueError('invalid PR identity')
    pull = event['pull_request']
    for side in ('base', 'head'):
        if check_sha(pull[side]['sha']) == '0' * 40:
            raise ValueError('PR requires live commits')
        repository_name(pull[side]['repo']['full_name'])
        if not isinstance(pull[side]['ref'], str) or not pull[side]['ref']:
            raise ValueError('missing PR branch')
    if pull['base']['repo']['full_name'] != repository:
        raise ValueError('PR target repository differs')
    if pull['base']['ref'] == 'develop':
        return False
    if (pull['base']['ref'] == 'main' and pull['head']['ref'] == 'develop'
            and pull['head']['repo']['full_name'] == repository):
        return True
    raise ValueError('CI PR must target develop or release develop to main')
