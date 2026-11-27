"""Publish fixed-role initialization observations from private final captures."""
from collections import Counter
import hashlib
import json

from diagnostics import command_outcome
from setup_evidence import EVENTS, REASONS, known, summarize_pods

ROLES = {'am_a': 'am-a-0', 'am_b': 'am-b-0', 'mongo': 'mongo',
         'identity': 'identity', 'provider': 'provider', 'api_proxy': 'api-proxy',
         'storage': 'storage', 'runtime_role': 'am-role'}


def _fixture_items(directory, name):
    path = directory / (name + '.final.json')
    if path.is_symlink() or not path.is_file():
        return None
    rows = json.loads(path.read_bytes())['items']
    if not isinstance(rows, list) or len(rows) > 200 or not all(isinstance(row, dict) for row in rows):
        raise ValueError('fixture_resource_sample_bound')
    return rows


def summarize_runtime_failure(directory):
    """Project the runtime objects captured before the journey retires them."""
    samples = sorted((path for path in directory.glob('diagnosis-*')
                      if path.name.removeprefix('diagnosis-').isdigit()
                      and path.is_dir() and not path.is_symlink()), reverse=True)
    if not samples:
        return None
    sample = samples[0]
    result = {'scope': 'before_public_retirement'}
    pods = _fixture_items(sample, 'pods')
    if pods is not None:
        runtime = [pod for pod in pods if any(container.get('name') == 'runtime'
                   for container in pod.get('spec', {}).get('containers', []))]
        result['runtime_pods'] = summarize_pods({'items': runtime})
        conditions = {'True', 'False', 'Unknown'}
        result['pod_scheduled'] = dict(sorted(Counter(
            known(next((c.get('status') for c in pod.get('status', {}).get('conditions', [])
                        if c.get('type') == 'PodScheduled'), None), conditions)
            for pod in runtime).items()))
    pvcs = _fixture_items(sample, 'pvc')
    if pvcs is not None:
        result['home_volume_phases'] = dict(sorted(Counter(
            known(row.get('status', {}).get('phase'), {'Pending', 'Bound', 'Lost'})
            for row in pvcs).items()))
    return result


def summarize(directory):
    result = {'scope': 'final_bounded_fixture_samples', 'complete_history': False,
              'root_cause_inferred': False, 'roles': {}, 'files': {}}

    def read(name):
        path = directory / name
        if not path.is_file() or path.is_symlink():
            return None
        raw = path.read_bytes()
        result['files'][name] = {'sha256': hashlib.sha256(raw).hexdigest(), 'bytes': len(raw)}
        return raw

    pods = []
    try:
        raw = read('pods.final.json')
        if raw is not None:
            pods = json.loads(raw)['items']
            if not isinstance(pods, list) or len(pods) > 200:
                raise ValueError('fixture_pod_sample_bound')
    except (ValueError, TypeError, KeyError, RecursionError):
        result['pod_sample_invalid'] = True
        pods = []
    for role, name in ROLES.items():
        value = {'observed': False}
        try:
            matches = [p for p in pods if isinstance(p, dict)
                       and isinstance(p.get('metadata'), dict) and p['metadata'].get('name') == name]
            if len(matches) == 1:
                pod = matches[0]
                value['pod'] = summarize_pods({'items': [pod]})
                status = pod['status']
                value['ready'] = any(c.get('type') == 'Ready' and c.get('status') == 'True'
                                     for c in status.get('conditions', []))
                counts = [c.get('restartCount', 0) for c in status.get('containerStatuses', [])]
                if not all(type(n) is int and 0 <= n < 1000000 for n in counts):
                    raise ValueError('fixture_restart_count_invalid')
                value['restarts'] = sum(counts)
                if role.startswith('am_'):
                    for container in status.get('containerStatuses', []):
                        previous = container.get('lastState', {}).get('terminated')
                        if previous is not None:
                            exit_code = previous.get('exitCode')
                            value['previous_termination'] = {
                                'reason': known(previous.get('reason'), REASONS),
                                'exit_code': exit_code if type(exit_code) is int and 0 <= exit_code <= 255 else None,
                            }
                value['observed'] = True
            elif matches:
                value['duplicate_role'] = True
        except (AttributeError, TypeError, KeyError, ValueError):
            value = {'observed': False, 'invalid_pod_sample': True}
        log_name = name + ('.full.private.log' if role.startswith('am_') else '.private.log')
        raw = read(log_name)
        if raw is not None:
            value['log_signals'] = command_outcome('fixture', 'other', None, 0, raw)['signals']
        if role.startswith('am_'):
            previous_log = read(name + '.previous.private.log')
            if previous_log is not None:
                value['previous_log_signals'] = command_outcome('fixture', 'other', None, 0, previous_log)['signals']
        result['roles'][role] = value
    try:
        raw = read('events.final.json')
        if raw is not None:
            items = json.loads(raw)['items']
            if not isinstance(items, list) or len(items) > 200:
                raise ValueError('fixture_event_sample_bound')
            result['event_reasons'] = dict(sorted(Counter(
                known(item.get('reason'), EVENTS) for item in items).items()))
    except (AttributeError, ValueError, TypeError, KeyError, RecursionError):
        result['event_sample_invalid'] = True
    return result
