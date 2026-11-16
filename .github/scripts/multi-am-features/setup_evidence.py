"""Collect bounded setup state without publishing identities or messages."""
from collections import Counter
import json

PHASES = {'Pending', 'Running', 'Succeeded', 'Failed', 'Unknown'}
REASONS = {'ContainerCreating', 'CrashLoopBackOff', 'ImagePullBackOff', 'ErrImagePull',
           'CreateContainerConfigError', 'CreateContainerError', 'RunContainerError',
           'InvalidImageName', 'PodInitializing', 'Completed', 'Error', 'OOMKilled'}
EVENTS = {'FailedScheduling', 'FailedMount', 'FailedAttachVolume', 'Failed', 'BackOff',
          'Unhealthy', 'Pulling', 'Pulled', 'Created', 'Started', 'Killing'}


def known(value, allowed):
    return value if isinstance(value, str) and value in allowed else 'Other'


def summarize_pods(payload):
    phases, states = Counter(), Counter()
    items = payload.get('items', [])
    if not isinstance(items, list) or len(items) > 200:
        raise ValueError('setup_pod_sample_bound')
    for pod in items:
        status = pod.get('status', {})
        phases[known(status.get('phase'), PHASES)] += 1
        for container in status.get('initContainerStatuses', []) + status.get('containerStatuses', []):
            state = container.get('state', {})
            for kind in ['waiting', 'running', 'terminated']:
                if kind in state:
                    details = state[kind]
                    reason = 'Running' if kind == 'running' else known(details.get('reason'), REASONS)
                    code = details.get('exitCode') if kind == 'terminated' else None
                    code = code if type(code) is int and 0 <= code <= 255 else None
                    states[kind, reason, code] += 1
    return {'pods': len(items), 'phases': dict(sorted(phases.items())),
            'container_states': [{'state': kind, 'reason': reason, 'exit_code': code, 'count': count}
                for (kind, reason, code), count in sorted(states.items(), key=lambda x: str(x[0]))]}


def capture(infrastructure):
    result = {'scope': 'bounded_setup_samples', 'complete_history': False, 'namespaces': {}}
    # A random cluster name alone cannot authorize observation of a reused node.
    infrastructure.bind_node()
    for label, namespace in [('controller', 'agent-sandbox-system'), ('storage', 'cma-native-storage')]:
        summary = {}
        infrastructure.stage = 'setup_capture_' + label
        for resource in ['pods', 'events']:
            try:
                value = json.loads(infrastructure.k(['-n', namespace, 'get', resource,
                    '--chunk-size=200', '-o', 'json'], timeout=35))
                if resource == 'pods':
                    summary['pod_sample'] = summarize_pods(value)
                else:
                    items = value.get('items', [])
                    if not isinstance(items, list) or len(items) > 200:
                        raise ValueError('setup_event_sample_bound')
                    summary['event_reasons'] = dict(sorted(Counter(
                        known(item.get('reason'), EVENTS) for item in items).items()))
            except Exception:
                summary[resource + '_capture_failed'] = True
        target = ('deployment/agent-sandbox-controller' if label == 'controller'
                  else 'statefulset/csi-hostpathplugin')
        try:
            infrastructure.k(['-n', namespace, 'logs', target, '--all-containers=true',
                '--prefix=true', '--tail=80', '--limit-bytes=32768'], timeout=35)
        except Exception:
            summary['logs_capture_failed'] = True
        result['namespaces'][label] = summary
    return result
