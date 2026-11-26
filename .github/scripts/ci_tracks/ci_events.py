"""Validate the CI event boundary independently of CD comparison policy."""

import os
import re

if __package__:
    from .git_comparison import check_sha
else:
    from git_comparison import check_sha

# GitHub names each merge-queue entry's branch after its base branch and PR.
# Only the develop queue is supported; main still merges reviewed release PRs.
QUEUE_REF = re.compile(r'refs/heads/gh-readonly-queue/develop/pr-([1-9][0-9]*)-[0-9a-f]{40}')
# A release is proposed from develop itself or from a release branch cut from
# one develop commit, so its tested head stops moving while develop does. The
# release gate proves a release branch's head is on develop's first-parent
# history; this only admits the name.
RELEASE_BRANCH = re.compile(r'release/[0-9A-Za-z][0-9A-Za-z._-]{0,62}')
CANDIDATE_REF_PREFIX = 'refs/heads/tl/release/'
VERIFICATION_EVENTS = {'schedule', 'workflow_dispatch'}


def verification_head():
    head = check_sha(os.environ['GITHUB_SHA'])
    if os.environ['GITHUB_REF'] != 'refs/heads/develop' or head == '0' * 40:
        raise ValueError('full CI verification requires a live develop commit')
    return head


def release_head(ref):
    return ref == 'develop' or (isinstance(ref, str) and RELEASE_BRANCH.fullmatch(ref) is not None)


def repository_name(value):
    if not isinstance(value, str) or not re.fullmatch(r'[A-Za-z0-9_.-]+/[A-Za-z0-9_.-]+', value):
        raise ValueError('invalid repository identity')
    return value


def queued_pull_number(event):
    """Return the PR a develop merge-queue entry tests, from its exact group ref."""
    if event.get('action') != 'checks_requested':
        raise ValueError('merge group must request checks')
    group = event['merge_group']
    for key in ('base_sha', 'head_sha'):
        if check_sha(group[key]) == '0' * 40:
            raise ValueError('merge group requires live commits')
    if group['base_sha'] == group['head_sha']:
        raise ValueError('merge group adds no commit')
    if group['base_ref'] != 'refs/heads/develop':
        raise ValueError('CI merge group must target develop')
    match = QUEUE_REF.fullmatch(group['head_ref']) if isinstance(group['head_ref'], str) else None
    if match is None:
        raise ValueError('merge group ref is not a develop queue entry')
    return int(match.group(1))


def full_ci_required(event, event_name, repository):
    repository_name(repository)
    if event['repository']['full_name'] != repository:
        raise ValueError('event repository differs')
    if event_name in VERIFICATION_EVENTS:
        verification_head()
        return True
    if event_name == 'push':
        ref = event['ref']
        candidate = (isinstance(ref, str) and ref.startswith(CANDIDATE_REF_PREFIX)
                     and len(ref) > len(CANDIDATE_REF_PREFIX))
        if (ref != 'refs/heads/develop' and not candidate) or event['deleted'] is not False:
            raise ValueError('CI push must update develop or an isolated release candidate')
        check_sha(event['before'])
        if check_sha(event['after']) == '0' * 40:
            raise ValueError('CI push requires a live commit')
        return True
    if event_name == 'merge_group':
        # A queue entry is the develop PR it names, tested on top of the queue
        # ahead of it: the same path selection and version label apply.
        queued_pull_number(event)
        return False
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
    if (pull['base']['ref'] == 'main' and release_head(pull['head']['ref'])
            and pull['head']['repo']['full_name'] == repository):
        return True
    raise ValueError('CI PR must target develop or release develop or a release branch to main')
