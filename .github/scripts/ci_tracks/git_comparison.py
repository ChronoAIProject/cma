"""Read exact comparison identities even at a shallow clone boundary."""

import re
import subprocess


def check_sha(ref):
    if not isinstance(ref, str) or not re.fullmatch(r'[0-9a-f]{40}', ref):
        raise ValueError('invalid comparison SHA')
    return ref


def git(root, *args):
    return subprocess.run(['git', *args], cwd=root, check=True,
                          capture_output=True, timeout=60).stdout


def first_parent(root, revision):
    # rev-parse HEAD^1 cannot traverse the graft at a shallow boundary. The
    # immutable commit still records its actual parent; never invent a base.
    raw = git(root, 'cat-file', 'commit', check_sha(revision))
    parents = [line[7:].decode('ascii') for line in raw.split(b'\n\n', 1)[0].splitlines()
               if line.startswith(b'parent ')]
    return check_sha(parents[0]) if parents else None


def shallow(root):
    value = git(root, 'rev-parse', '--is-shallow-repository').strip()
    if value not in (b'true', b'false'):
        raise ValueError('invalid history completeness result')
    return value == b'true'


def comparison_details(root, event, event_name):
    if event_name == 'push':
        head = check_sha(event['after'])
        base = first_parent(root, head)
        if base is None:
            return head, head, 'comparison-boundary-not-found'
        if shallow(root):
            return base, head, 'shallow-history'
        git(root, 'cat-file', '-e', base + '^{commit}')
        return base, head, None
    if event_name != 'pull_request':
        raise ValueError('unsupported selector event')
    pull = event['pull_request']
    head, target = check_sha(pull['head']['sha']), check_sha(pull['base']['sha'])
    git(root, 'cat-file', '-e', head + '^{commit}')
    if shallow(root):
        return target, head, 'shallow-history'
    git(root, 'cat-file', '-e', target + '^{commit}')
    try:
        base = check_sha(git(root, 'merge-base', target, head).decode().strip())
    except subprocess.CalledProcessError as error:
        if error.returncode != 1:
            raise
        # Both supplied commits exist; no common comparison boundary is an
        # analysis limitation. Release/adoption authority is validated elsewhere.
        return target, head, 'comparison-boundary-not-found'
    return base, head, None
