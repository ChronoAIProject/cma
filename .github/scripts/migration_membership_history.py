"""Bind each finite observation to the exact earlier stages in this process."""
from copy import deepcopy
import hashlib
import json
import os
import stat

from migration_catalog import STAGES
from migration_membership_validation import compare, require, validate
from migration_release_probe import STAGE_ROLES


def read_bounded(path):
    """Read one regular evidence file without following a substituted symlink."""
    try:
        descriptor = os.open(path, os.O_RDONLY | os.O_NOFOLLOW | os.O_NONBLOCK)
    except OSError as error:
        raise ValueError('membership migration: evidence file unavailable or symlink') from error
    try:
        require(stat.S_ISREG(os.fstat(descriptor).st_mode), 'evidence is not a regular file')
        with os.fdopen(descriptor, 'rb', closefd=False) as stream:
            raw = stream.read(65537)
    finally:
        os.close(descriptor)
    require(len(raw) <= 65536, 'evidence exceeds byte bound')
    return raw


def begin(campaign, stage, source, image):
    require(stage in STAGES, 'unknown stage')
    history = getattr(campaign, '_membership_history', [])
    require([item['receipt']['stage'] for item in history] == list(STAGES[:STAGES.index(stage)]),
            'membership stages skipped or repeated')
    require(source == campaign.active_reader.get('source'), 'source differs from booted reader')
    require(campaign.reader_artifacts[STAGE_ROLES[stage]] == {'source': source, 'image': image},
            'reader artifact changed')
    return history


def observations(campaign, history, current):
    known = set()
    comparisons = []
    for item in history:
        receipt = item['receipt']
        stage = receipt['stage']
        binding = read_bounded(campaign.output / ('membership-' + stage + '.binding.json'))
        require(hashlib.sha256(binding).hexdigest() == item['binding_sha256']
                and json.loads(binding) == receipt, 'prior membership binding changed')
        raw = read_bounded(campaign.output / ('membership-' + stage + '.private.json'))
        require(hashlib.sha256(raw).hexdigest() == receipt['sha256'],
                'prior membership snapshot hash differs')
        previous = validate(json.loads(raw), receipt['membership_pod'])
        known.add(previous['member']['_id'])
        comparisons.append({'from_stage': stage, **compare(previous, current)})
    require(set(current['members']) <= known | {current['member']['_id']},
            'unexplained process incarnation')
    if history:
        previous = history[-1]['receipt']
        same_pod = previous['membership_pod']['uid'] == current['pod']['uid']
        if not same_pod:
            require(current['member']['_id'] not in known, 'retired incarnation reused')
    return comparisons


def retain(campaign, history, receipt):
    binding = campaign.output / ('membership-' + receipt['stage'] + '.binding.json')
    raw = (json.dumps(receipt, indent=2) + '\n').encode()
    # One successful stage gets one binding; duplicate execution never replaces
    # the frozen receipt that the next image must consume.
    descriptor = os.open(binding, os.O_WRONLY | os.O_CREAT | os.O_EXCL, 0o600)
    with os.fdopen(descriptor, 'wb') as stream:
        stream.write(raw)
    campaign._membership_history = [*history, {
        'receipt': deepcopy(receipt), 'binding_sha256': hashlib.sha256(raw).hexdigest()}]
