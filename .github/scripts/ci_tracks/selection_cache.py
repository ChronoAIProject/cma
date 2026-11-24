"""Bounded selection records; the workflow supplies trusted develop cache state."""

import hashlib
import json
import re
import sys
if __package__:
    from .deployable_manifest import CURRENT
    from .uncertainty import REASONS
else:
    from deployable_manifest import CURRENT
    from uncertainty import REASONS

SCHEMA = 'cma-selection-cache-v3'
FIELDS = {'base', 'head', 'repo', 'am', 'eg', 'web', 'reasons', 'affected', 'am_packages',
          'eg_packages', 'test_tracks', 'deployables', 'changed_path_count'}
IMAGES = frozenset(CURRENT)
# The staging producer runs batch_selection. These scripts serve the CI
# entrypoints and cannot change a staging comparison or its input map.
CI_ONLY_SCRIPTS = frozenset({'ci_events.py', 'version_label.py'})


def canonical(value):
    return json.dumps(value, sort_keys=True, separators=(',', ':')).encode()


def workflow_producer(data, job):
    # Keep YAML scalar values and tags intact. In particular, "on" and "true"
    # would both become the same Python key with a YAML 1.1 object loader.
    import yaml

    def fields(node):
        if not isinstance(node, yaml.MappingNode):
            raise ValueError('invalid selector producer mapping')
        items = {}
        for key, value in node.value:
            if not isinstance(key, yaml.ScalarNode) or key.value == '<<' or key.value in items:
                raise ValueError('ambiguous selector producer mapping')
            items[key.value] = (key, value)
        return items

    document = yaml.compose(data, Loader=yaml.SafeLoader)
    top = fields(document)
    top.pop('concurrency', None)
    if 'jobs' not in top:
        raise ValueError('missing selector producer jobs')
    jobs_key, jobs = top['jobs']
    entries = fields(jobs)
    if job not in entries:
        raise ValueError('missing selector producer job')
    key, producer = entries[job]
    settings = fields(producer)
    settings.pop('concurrency', None)
    selected = yaml.MappingNode(producer.tag, list(settings.values()))
    # A producer depending on another job can consume its outputs. In that
    # case keep the full job graph in the key, including upstream steps.
    if 'needs' not in settings:
        top['jobs'] = (jobs_key, yaml.MappingNode(jobs.tag, [(key, selected)]))
    return yaml.serialize(yaml.MappingNode(document.tag, list(top.values())), canonical=True).encode()


def fingerprint(root):
    # A policy refresh must miss before a historical decision or input map
    # is loaded, even when a tracked manifest was omitted from its archive.
    paths = sorted(
        path for path in (root / '.github/scripts/ci_tracks').glob('*.py')
        if path.name not in CI_ONLY_SCRIPTS and not path.name.startswith('test_')
    )
    paths += [root / '.github/scripts/ci_tracks/requirements.txt',
              root / 'api/rust-toolchain.toml', root / '.github/scripts/deployables.json',
              root / '.github/scripts/staging_cd_comparisons.py', root / '.github/scripts/staging_cd_admission.py',
              root / '.github/scripts/staging_cd_selection_cache.py']
    workflows = {root / '.github/workflows/staging-cd.yml': 'plan',
                 root / '.github/workflows/ci.yml': 'changes'}
    paths.extend(workflows)
    digest = hashlib.sha256(canonical([SCHEMA, *sys.version_info[:3]]))
    for path in paths:
        if path.is_symlink() or not path.is_file():
            raise ValueError('invalid selector fingerprint input')
        data = path.read_bytes()
        if path in workflows:
            data = workflow_producer(data, workflows[path])
        digest.update(path.relative_to(root).as_posix().encode() + b'\0' + data + b'\0')
    return digest.hexdigest()


def validate_result(value, base, head):
    if not isinstance(value, dict) or set(value) != FIELDS or value['base'] != base or value['head'] != head:
        return False
    if any(type(value[key]) is not bool for key in ('repo', 'am', 'eg', 'web')) or not value['repo']:
        return False
    if value['test_tracks'] != {key: value[key] for key in ('repo', 'am', 'eg', 'web')}:
        return False
    for key in ('reasons', 'affected', 'am_packages', 'eg_packages', 'deployables'):
        rows = value[key]
        if not isinstance(rows, list) or any(not isinstance(row, str) or not re.fullmatch(r'[A-Za-z0-9_.-]+', row) for row in rows):
            return False
        if rows != sorted(set(rows)):
            return False
    if set(value['reasons']) & REASONS and (set(value['deployables']) != IMAGES
            or not all(value[key] for key in ('repo', 'am', 'eg', 'web'))):
        return False
    return (set(value['deployables']) <= IMAGES and type(value['changed_path_count']) is int
            and 0 <= value['changed_path_count'] <= 1000000)


class SelectionCache:
    def __init__(self, directory, policy):
        self.directory, self.policy = directory, policy
        self.hits = self.misses = self.input_hits = self.input_misses = 0
        if directory.is_symlink() or not re.fullmatch(r'[0-9a-f]{64}', policy):
            raise ValueError('invalid selection cache context')
        directory.mkdir(parents=True, exist_ok=True)

    def load(self, base, head, context='source-manifests'):
        try:
            path = self.path(head)
            if path.is_symlink() or path.stat().st_size > 65536:
                raise ValueError('invalid selection cache record')
            record = json.loads(path.read_bytes())
            value = record['result']
            if (set(record) != {'schema', 'policy', 'context', 'checksum', 'result'} or record['schema'] != SCHEMA
                    or record['policy'] != self.policy or record['context'] != context
                    or record['checksum'] != hashlib.sha256(canonical(value)).hexdigest()
                    or not validate_result(value, base, head)):
                raise ValueError('selection cache identity differs')
        except (OSError, ValueError, KeyError, TypeError):
            # Availability or malformed state never becomes an empty selection.
            self.misses += 1
            return None
        self.hits += 1
        return value

    def path(self, head):
        if not isinstance(head, str) or not re.fullmatch(r'[0-9a-f]{40}', head):
            raise ValueError('invalid selection cache revision')
        return self.directory / (head + '.json')

    def save(self, result, context='source-manifests'):
        if not validate_result(result, result.get('base'), result.get('head')):
            raise ValueError('cannot cache incomplete selection')
        record = {'schema': SCHEMA, 'policy': self.policy, 'context': context, 'result': result,
                  'checksum': hashlib.sha256(canonical(result)).hexdigest()}
        path = self.path(result['head'])
        if path.is_symlink():
            path.unlink()
        path.write_bytes(canonical(record) + b'\n')

    def load_inputs(self, revision, context):
        path = self.directory / ('inputs-' + self.path(revision).name)
        try:
            if path.is_symlink() or path.stat().st_size > 8 * 1024 * 1024:
                raise ValueError('invalid cached inputs')
            record = json.loads(path.read_bytes())
            inputs = record['inputs']
            if (set(record) != {'schema', 'policy', 'revision', 'context', 'checksum', 'inputs'}
                    or record['schema'] != SCHEMA or record['policy'] != self.policy
                    or record['revision'] != revision or record['context'] != context
                    or record['checksum'] != hashlib.sha256(canonical(inputs)).hexdigest()
                    or not valid_inputs(inputs)):
                raise ValueError('cached input identity differs')
        except (OSError, ValueError, KeyError, TypeError):
            self.input_misses += 1
            return None
        self.input_hits += 1
        return {name: set(paths) for name, paths in inputs.items()}

    def save_inputs(self, revision, context, inputs):
        # Uncertain analysis is deliberately never reused as an input map.
        if inputs is None:
            return
        value = {name: sorted(paths) for name, paths in inputs.items()}
        if not valid_inputs(value):
            raise ValueError('cannot cache invalid inputs')
        record = {'schema': SCHEMA, 'policy': self.policy, 'revision': revision,
                  'context': context, 'inputs': value,
                  'checksum': hashlib.sha256(canonical(value)).hexdigest()}
        data = canonical(record) + b'\n'
        if len(data) > 8 * 1024 * 1024:
            raise ValueError('cached inputs exceed limit')
        path = self.directory / ('inputs-' + self.path(revision).name)
        if path.is_symlink():
            path.unlink()
        path.write_bytes(data)

    def retain(self, revisions):
        allowed = {revision + '.json' for revision in revisions}
        if revisions:
            allowed.add('inputs-' + revisions[-1] + '.json')
        # Keep only the current bounded reconciliation. Unexpected restored
        # paths are rejected rather than followed or republished.
        for path in self.directory.iterdir():
            if path.is_symlink() or not path.is_file() or not re.fullmatch(r'(?:inputs-)?[0-9a-f]{40}\.json', path.name):
                raise ValueError('invalid selection cache entry')
            if path.name not in allowed:
                path.unlink()


def valid_inputs(value):
    if not isinstance(value, dict) or set(value) != IMAGES:
        return False
    for paths in value.values():
        if (not isinstance(paths, list) or any(not isinstance(path, str) or not path
                or path.startswith('/') or '..' in path.split('/') or '\x00' in path for path in paths)
                or paths != sorted(set(paths))):
            return False
    return True
