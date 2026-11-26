"""Publish fixed-role initialization observations from private final captures."""
from collections import Counter
import hashlib
import json

from diagnostics import command_outcome
from setup_evidence import EVENTS, known, summarize_pods

ROLES = {'am_a': 'am-a-0', 'am_b': 'am-b-0', 'mongo': 'mongo',
         'identity': 'identity', 'provider': 'provider', 'api_proxy': 'api-proxy',
         'storage': 'storage', 'runtime_role': 'am-role'}


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
                value['observed'] = True
            elif matches:
                value['duplicate_role'] = True
        except (AttributeError, TypeError, KeyError, ValueError):
            value = {'observed': False, 'invalid_pod_sample': True}
        log_name = name + ('.full.private.log' if role.startswith('am_') else '.private.log')
        raw = read(log_name)
        if raw is not None:
            value['log_signals'] = command_outcome('fixture', 'other', None, 0, raw)['signals']
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
