"""Sequential S3b CPU-target exercise through the existing approval/CAS controller."""
import copy
import datetime as dt
from pathlib import Path
import re

import checkpoints
import rules as g

TARGET_PATH = '/spec/metrics/0/resource/target/averageUtilization'


def original_spec(role):
    filename = 'S3-api-hpa.json' if role == 'am' else 'S3-cmaeg-hpa.json'
    return g.load(Path(__file__).with_name('templates') / filename)['manifest']['spec']


def utilization(obj):
    status = obj.get('status', {})
    g.require(status.get('observedGeneration') == obj['metadata']['generation'] and
              all(any(c.get('type') == kind and c.get('status') == 'True'
                      for c in status.get('conditions', []))
                  for kind in ('AbleToScale', 'ScalingActive')),
              'S3b_HPA_conditions_not_current')
    metrics = status.get('currentMetrics', [])
    g.require(len(metrics) == 1 and metrics[0].get('type') == 'Resource' and
              metrics[0].get('resource', {}).get('name') == 'cpu', 'S3b_CPU_metric_required')
    value = metrics[0]['resource'].get('current', {}).get('averageUtilization')
    g.require(type(value) is int and value >= 12, 'S3b_fresh_utilization_at_least_12_required')
    return value


def age(timestamp):
    g.require(isinstance(timestamp, str), 'S3b_timestamp_required')
    instant = dt.datetime.fromisoformat(timestamp.replace('Z', '+00:00'))
    g.require(instant.tzinfo is not None, 'S3b_timestamp_timezone_required')
    return (dt.datetime.now(dt.timezone.utc) - instant).total_seconds()


def stable_pairs(release):
    bindings, cohorts = {}, {}
    for role, name in (('am', 'cma-agent-manager'), ('eg', 'cma-event-gateway')):
        obj = g.get('Deployment', name)
        g.require(obj is not None, 'S3b_Deployment_required')
        pods = checkpoints.stable(obj, 2, distinct=True)
        container = next(c for c in obj['spec']['template']['spec']['containers']
                         if c['name'] == g.TARGETS[name])
        g.require(container['image'] == release['images'][role] and
                  obj['spec']['template']['spec'].get('terminationGracePeriodSeconds') == 760 and
                  all(p['spec'].get('terminationGracePeriodSeconds') == 760 for p in pods),
                  'S3b_selected_image_and_actual_760_grace_required')
        request = container.get('resources', {}).get('requests', {}).get('cpu')
        g.require(g.cpu(request) == g.cpu('4' if role == 'am' else '250m'),
                  'S3b_original_CPU_request_required')
        for pod in pods:
            condition = next(c for c in pod['status']['conditions'] if c.get('type') == 'Ready')
            g.require(age(condition.get('lastTransitionTime')) >= 600,
                      'S3b_ten_minutes_ready_required')
        other_hpa = g.get('HorizontalPodAutoscaler', name)
        g.require(other_hpa is not None and other_hpa['spec'] == original_spec(role),
                  'S3b_both_original_policies_required')
        bindings[role], cohorts[role] = g.binding(obj), pods
    return bindings, cohorts


def fresh_metrics(name, pods):
    route = ('/apis/metrics.k8s.io/v1beta1/namespaces/' + g.NAMESPACE +
             '/pods?labelSelector=app.kubernetes.io%2Fname%3D' + name)
    samples = g.command(g.K + ['get', '--raw', route])['items']
    for pod in pods:
        matches = [m for m in samples if m['metadata']['name'] == pod['metadata']['name']]
        g.require(len(matches) == 1 and 0 <= age(matches[0].get('timestamp')) <= 60,
                  'S3b_fresh_PodMetrics_required')
        window = matches[0].get('window', '')
        g.require(isinstance(window, str) and re.fullmatch(r'[0-9]+(?:\.[0-9]+)?s', window)
                  and 0 < float(window[:-1]) <= 60, 'S3b_bounded_metric_window_required')
        values = matches[0].get('containers', [])
        g.require(sorted(c['name'] for c in values) == sorted(c['name'] for c in pod['spec']['containers'])
                  and all(g.cpu(c.get('usage', {}).get('cpu')) >= 0 for c in values),
                  'S3b_complete_PodMetrics_required')


def build(role, action, release, created):
    name = 'cma-agent-manager' if role == 'am' else 'cma-event-gateway'
    g.require(created is not None, 'S3b_confirmed_prior_result_required')
    result = g.load(created)
    prior = result.get('operation', {})
    g.require(action in ('lower-target', 'restore-target'), 'S3b_action_required')
    g.require(result.get('state') == 'APPLIED' and result.get('source') == release['source']
              and prior.get('kind') == 'HorizontalPodAutoscaler' and prior.get('name') == name
              and (prior.get('action') == 'create' if action == 'lower-target' else
                   prior.get('id') == 's3-' + role + '-lower-target'
                   and prior.get('hpa_exercise') == 'lower-target'),
              'S3b_confirmed_owned_result_required')
    obj = g.get('HorizontalPodAutoscaler', name)
    g.same(obj, result.get('after'))
    op = dict(id=f's3-{role}-{action}', stage='S3', context=g.CONTEXT, namespace=g.NAMESPACE,
              kind='HorizontalPodAutoscaler', name=name, action='patch', role=role,
              hpa_exercise=action, expected_before=g.binding(obj))
    if action == 'lower-target':
        g.require(obj['spec'] == original_spec(role), 'S3b_original_HPA_spec_required')
        value = utilization(obj)
        lower = max(1, value * 5 // 6)
        original = obj['spec']['metrics'][0]['resource']['target']['averageUtilization']
        g.require(lower < original, 'S3b_target_must_be_lower_than_original')
        bindings, cohorts = stable_pairs(release)
        fresh_metrics(name, cohorts[role])
        spec = copy.deepcopy(obj['spec'])
        spec['metrics'][0]['resource']['target']['averageUtilization'] = lower
        op.update(original_spec=copy.deepcopy(obj['spec']), sampled_utilization=value,
                  deployment_bindings=bindings, changes={'spec': spec})
    else:
        saved = prior.get('original_spec')
        g.require(saved == original_spec(role) and obj['spec'] == prior['changes']['spec'],
                  'S3b_restore_spec_drift')
        op['changes'] = {'spec': copy.deepcopy(saved)}
    return op


def checkpoint(obj, op, release):
    g.same(obj, op['expected_before'])
    if op['hpa_exercise'] == 'restore-target':
        return  # Failed metrics/readiness must not prevent the guarded target restore.
    g.require(utilization(obj) == op['sampled_utilization'], 'S3b_metric_changed_replan')
    bindings, cohorts = stable_pairs(release)
    g.require(bindings == op['deployment_bindings'], 'S3b_Deployment_changed')
    fresh_metrics(op['name'], cohorts[op['role']])
