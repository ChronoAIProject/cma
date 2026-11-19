"""Prepare an Actions cache restricted by the backend to protected develop."""

import json
import os
from pathlib import Path

from ci_tracks.selection_cache import fingerprint
from staging_cd_admission import event_identity
from staging_cd_common import require
from staging_cd_resolve import output
from staging_cd_runtime import run_private


def prepare(root, env):
    event_identity(env)
    require(env['GITHUB_EVENT_NAME'] == 'push', 'Selection cache requires a develop push')
    require(env.get('GITHUB_WORKFLOW_REF') == 'ChronoAIProject/cma/.github/workflows/staging-cd.yml@refs/heads/develop',
            'Selection cache workflow authority differs')
    # GitHub scopes Actions cache reads/writes to the authenticated workflow
    # ref. A develop push cannot restore a PR merge-ref cache, even if its key
    # matches. These checks supplement that service boundary, not replace it.
    temporary = Path(env['RUNNER_TEMP']).resolve(strict=True)
    directory = temporary / 'cma-selection-cache'
    require(not directory.is_symlink(), 'Selection cache directory is invalid')
    policy = fingerprint(root)
    prefix = 'cma-develop-selection-v2-' + policy + '-'
    output(env, 'prefix', prefix)
    output(env, 'key', prefix + env['GITHUB_SHA'] + '-' + env['GITHUB_RUN_ID'] + '-' + env['GITHUB_RUN_ATTEMPT'])
    output(env, 'directory', str(directory))
    with Path(env['GITHUB_ENV']).open('a') as exported:
        exported.write('CD_SELECTOR_CACHE=' + json.dumps({'directory': str(directory), 'policy': policy}) + '\n')
    return True


if __name__ == '__main__':
    raise SystemExit(run_private(lambda _masker: prepare(Path.cwd(), os.environ), None))
