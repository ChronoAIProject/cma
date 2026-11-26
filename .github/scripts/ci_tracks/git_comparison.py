"""Read exact comparison identities even at a shallow clone boundary."""

import io
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
    return commit_first_parent(raw)


def commit_first_parent(raw):
    for line in raw.split(b'\n\n', 1)[0].splitlines():
        if line.startswith(b'parent '):
            return check_sha(line[7:].decode('ascii'))
    return None


def first_parents(root, revisions):
    """Read each raw commit's parent in one Git process, including shallow grafts."""
    requests = [check_sha(revision).encode('ascii') for revision in revisions]
    raw = subprocess.run(['git', 'cat-file', '--batch'], cwd=root,
                         input=b'\n'.join(requests) + b'\n', check=True,
                         capture_output=True, timeout=60).stdout
    parents = {}
    with io.BytesIO(raw) as stream:
        for revision in requests:
            header = stream.readline().split()
            if len(header) != 3 or header[:2] != [revision, b'commit'] or not header[2].isdigit():
                raise ValueError('batch response is not the requested commit')
            size = int(header[2])
            commit = stream.read(size)
            if len(commit) != size or stream.read(1) != b'\n':
                raise ValueError('truncated commit batch')
            parents[revision.decode('ascii')] = commit_first_parent(commit)
        if stream.read(1):
            raise ValueError('unexpected extra commit batch data')
    return parents


def shallow(root):
    value = git(root, 'rev-parse', '--is-shallow-repository').strip()
    if value not in (b'true', b'false'):
        raise ValueError('invalid history completeness result')
    return value == b'true'


def comparison_details(root, event, event_name, *, allow_shallow_parent=False):
    if event_name == 'push':
        head = check_sha(event['after'])
        base = first_parent(root, head)
        if base is None:
            return head, head, 'comparison-boundary-not-found'
        # The CD batch has separately proved its bounded first-parent chain.
        # Comparing two available immutable trees needs no older ancestry.
        if shallow(root) and not allow_shallow_parent:
            return base, head, 'shallow-history'
        git(root, 'cat-file', '-e', base + '^{commit}')
        return base, head, None
    if event_name == 'merge_group':
        # The group's parent is develop or the queue entry ahead of it, so the
        # comparison is this entry's own change, tested on the cumulative tree.
        group = event['merge_group']
        head, target = check_sha(group['head_sha']), check_sha(group['base_sha'])
    elif event_name == 'pull_request':
        pull = event['pull_request']
        head, target = check_sha(pull['head']['sha']), check_sha(pull['base']['sha'])
    else:
        raise ValueError('unsupported selector event')
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
