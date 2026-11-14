"""Build one narrowly scoped operation, including state-specific recovery."""
import copy
import math
import re
from pathlib import Path

import rules as g
import checkpoints

TEMPLATES = Path(__file__).with_name('templates')
SUFFIXES = {'pause': 'pause', 'prepare': 'prepare-template.template',
            'stop': 'stop-legacy', 'unpause': 'unpause-zero', 'start': 'start-two',
            'pdb': 'pdb', 'one': 'rollback-one'}


def image(value, service):
    repo = 'cma-api' if service == 'am' else 'cma-cmaeg'
    g.require(isinstance(value, str) and re.fullmatch(
        r'us-central1-docker.pkg.dev/aelf-shared-k8s-prod/aelf-prod-registry/'
        + repo + r'@sha256:[0-9a-f]{64}', value), 'qualified_image_required')
    return value


def release(path):
    value = g.load(path)
    g.require(isinstance(value.get('source'), str) and
              re.fullmatch('[0-9a-f]{40}', value['source']) is not None,
              'final_source_required')
    for role in ('am', 'eg'):
        image(value.get('images', {}).get(role), role)
    return value


def base(stage, service, ident):
    name = 'cma-agent-manager' if service == 'am' else 'cma-event-gateway'
    return dict(id=ident, stage=stage, context=g.CONTEXT, namespace=g.NAMESPACE,
                kind='Deployment', name=name, action='patch')


def recovery_image(release, service):
    entry = release.get('recovery', {}).get(service, {})
    g.require(entry.get('compatible_mode_on') is True,
              'compatible_recovery_must_be_qualified_before_cutover')
    return image(entry.get('image'), service)


def abort_before_stop(stage, service, pause_plan, prepare_plan=None):
    """Undo only our inactive template, while the original one still serves."""
    g.require(pause_plan is not None, 'pause_plan_required')
    first = g.load(pause_plan)
    g.require(first['operation']['id'] == stage.lower() + '-pause', 'wrong_pause_plan')
    name = first['operation']['name']
    current = g.get('Deployment', name)
    g.require(current is not None and current['spec'].get('paused') is True
              and current['spec'].get('replicas') == 1, 'abort_only_before_old_stop')
    g.require(g.binding(current)['uid_sha256'] == first['before']['uid_sha256'],
              'original_deployment_changed')
    original = first['operation'].get('original_process')
    checkpoints.same_original_process(current, original)
    last = g.load(prepare_plan) if prepare_plan else first
    if prepare_plan:
        g.require(last['operation']['id'] == stage.lower() + '-prepare-paused-template'
                  and last['release']['source'] == first['release']['source']
                  and last['before']['uid_sha256'] == first['before']['uid_sha256'],
                  'wrong_prepare_plan')
    g.require(g.hash_json(current['spec']) == last['expected_spec_sha256'], 'abort_spec_drift')
    restored = copy.deepcopy(current)
    for plan in ([last, first] if prepare_plan else [first]):
        for item in reversed(plan['previous_fields']):
            if 'env_name' in item:
                container = next(c for c in restored['spec']['template']['spec']['containers']
                                 if c['name'] == item['container'])
                rows = container['env']
                matches = [i for i, row in enumerate(rows) if row['name'] == item['env_name']]
                g.require(len(matches) == 1, 'abort_env_drift')
                if item['present']:
                    rows[matches[0]] = item['value']
                else:
                    rows.pop(matches[0])
            else:
                target = restored
                for key in item['path'][:-1]:
                    target = target[key]
                key = item['path'][-1]
                if item['present']:
                    target[key] = item['value']
                else:
                    target.pop(key, None)
    g.require(g.hash_json(restored['spec']) == first['before']['spec_sha256'],
              'abort_does_not_restore_original_spec')
    g.require(g.template_identity(restored['spec']['template']) == original['template_sha256'],
              'abort_original_template_mismatch')
    op = base(stage, service, stage.lower() + '-abort-before-stop')
    op.update(abort_before_stop=True, expected_before=g.binding(current),
              original_process=original,
              restored_spec_sha256=first['before']['spec_sha256'],
              restore_fields=[p['previous_fields'] for p in
                              ([last, first] if prepare_plan else [first])])
    return op


def build(action, release, created=None, pause_plan=None, prepare_plan=None):
    parts = action.split('-', 1)
    g.require(len(parts) == 2 and parts[0] in ('s1', 's2', 's3'), 'unknown_action')
    stage, step = parts[0].upper(), parts[1]
    if stage == 'S3':
        return hpa(step, release, created)
    service = 'am' if stage == 'S1' else 'eg'
    if step in SUFFIXES:
        op = g.load(TEMPLATES / f'{stage}-{SUFFIXES[step]}.json')
        if step == 'prepare':
            recovery_image(release, service)
            op['changes']['container']['image'] = image(release['images'][service], service)
            if service == 'am':
                capacity = release.get('am_max_owned_agents')
                g.require(type(capacity) is int and capacity > 0, 'am_owned_capacity_required')
                for row in op['changes']['env']:
                    if row['name'] == 'CHRONO_AGENT_MANAGER_MAX_OWNED_AGENTS':
                        row['value'] = str(capacity)
        if step == 'stop':
            recovery_image(release, service)
            g.require(release.get('controller_order_approved') is True and
                      release.get('no_overlap_under_120s_proven') is True,
                      'cutover_order_outage_and_recovery_preconditions_required')
        return op
    if step == 'abort-before-stop':
        return abort_before_stop(stage, service, pause_plan, prepare_plan)
    op = base(stage, service, stage.lower() + '-' + step)
    if step == 'quiesce':
        op.update(emergency_quiesce=True,
                  changes={'deployment': {'paused': True, 'replicas': 0}})
    elif step == 'recovery-image':
        op.update(requires_zero=True, changes={'container': {
            'image': recovery_image(release, service)}})
    elif step == 'recover-one':
        op.update(requires_zero=True, recover_one=True,
                  changes={'deployment': {'replicas': 1}})
    else:
        raise RuntimeError('unknown_action')
    return op


def hpa(step, release, created):
    role, action = step.split('-', 1)
    if role == 'quota':
        return quota(action, created)
    g.require(role in ('am', 'eg'), 'wrong_HPA_role')
    name = 'cma-agent-manager' if role == 'am' else 'cma-event-gateway'
    if action == 'create':
        op = g.load(TEMPLATES / ('S3-api-hpa.json' if role == 'am' else 'S3-cmaeg-hpa.json'))
        timing = release.get('hpa', {}).get(role, {})
        p95, lifetime = timing.get('handoff_p95_seconds'), timing.get('hard_lifetime_seconds')
        g.require(all(type(x) in (int, float) and math.isfinite(x) and x >= 0
                      for x in (p95, lifetime)) and lifetime > 0, 'qualified_p95_and_hard_lifetime_required')
        period = 120
        g.require(p95 <= 90, 'handoff_p95_incompatible_with_fixed_120s_cadence')
        limit = timing.get('capacity_processes_per_service')
        g.require(type(limit) is int and limit >= 5 + math.ceil(lifetime / period)
                  and timing.get('cpu_signal_qualified') is True, 'HPA_capacity_or_CPU_not_qualified')
        op['manifest']['spec']['behavior']['scaleDown']['policies'][0]['periodSeconds'] = period
        return op
    g.require(action in ('freeze', 'delete', 'descend'), 'unknown_HPA_action')
    op = base('S3', role, f's3-{action}-{name}')
    if action == 'descend':
        g.require(g.get('HorizontalPodAutoscaler', name) is None, 'remove_HPA_before_descent')
        obj = g.get('Deployment', name)
        n = obj['spec'].get('replicas', 1)
        g.require(type(n) is int and 2 < n <= 5, 'descent_stops_at_two')
        op.update(id=f's3-descend-{name}-{n}-to-{n-1}',
                  changes={'deployment': {'replicas': n - 1}}, expected_before=g.binding(obj))
        return op
    g.require(created is not None, 'HPA_creation_result_required')
    result = g.load(created)
    obj = g.get('HorizontalPodAutoscaler', name)
    g.require(obj is not None and result.get('state') == 'APPLIED'
              and result['operation'].get('action') == 'create'
              and result['operation'].get('kind') == 'HorizontalPodAutoscaler'
              and result['operation'].get('name') == name
              and result.get('after', {}).get('uid_sha256') == g.binding(obj)['uid_sha256'],
              'only_exact_stage_created_HPA')
    op.update(kind='HorizontalPodAutoscaler', action='patch' if action == 'freeze' else 'delete',
              expected_before=g.binding(obj))
    if action == 'freeze':
        spec = copy.deepcopy(obj['spec'])
        for direction in ('scaleUp', 'scaleDown'):
            spec.setdefault('behavior', {}).setdefault(direction, {})['selectPolicy'] = 'Disabled'
        op['changes'] = {'spec': spec}
    else:
        g.require(all(obj['spec'].get('behavior', {}).get(d, {}).get('selectPolicy') == 'Disabled'
                      for d in ('scaleUp', 'scaleDown')), 'freeze_both_HPA_directions_first')
    return op


def quota(action, created):
    g.require(action in ('raise', 'restore'), 'unknown_quota_action')
    op = dict(id='s3-quota-' + action, stage='S3', context=g.CONTEXT,
              namespace=g.NAMESPACE, kind='ResourceQuota', name='sandbox-quota',
              action='patch', changes={'cpu': dict(g.QUOTA_AFTER)})
    if action == 'restore':
        g.require(created is not None, 'quota_raise_result_required')
        result = g.load(created)
        prior = result.get('operation', {})
        g.require(result.get('state') == 'APPLIED' and prior.get('id') == 's3-quota-raise'
                  and prior.get('kind') == 'ResourceQuota' and prior.get('name') == 'sandbox-quota',
                  'confirmed_quota_raise_required')
        current = g.get('ResourceQuota', 'sandbox-quota')
        g.require(current is not None and g.binding(current) == result.get('after'),
                  'quota_identity_or_spec_changed_after_raise')
        before = result.get('quota_before_spec')
        g.require(isinstance(before, dict) and g.quota_cpu(before, g.QUOTA_BEFORE),
                  'saved_quota_before_spec_required')
        op['quota_before_spec'] = before
        op['changes']['cpu'] = {k: before['hard'][k] for k in g.QUOTA_BEFORE}
        op['expected_before'] = g.binding(current)
    return op


def transformed(obj, op):
    if not op.get('abort_before_stop'):
        return g.transform(obj, op)
    restored = copy.deepcopy(obj)
    for group in op['restore_fields']:
        for item in reversed(group):
            if 'env_name' in item:
                c = next(c for c in restored['spec']['template']['spec']['containers']
                         if c['name'] == item['container'])
                i = next(i for i, e in enumerate(c['env']) if e['name'] == item['env_name'])
                if item['present']:
                    c['env'][i] = item['value']
                else:
                    c['env'].pop(i)
            else:
                target = restored
                for key in item['path'][:-1]:
                    target = target[key]
                if item['present']:
                    target[item['path'][-1]] = item['value']
                else:
                    target.pop(item['path'][-1], None)
    g.require(g.hash_json(restored['spec']) == op['restored_spec_sha256'], 'abort_restore_drift')
    if op['name'] == 'cma-event-gateway':
        container = next(c for c in restored['spec']['template']['spec']['containers']
                         if c['name'] == 'cmaeg')
        g.require('exec' not in container.get('lifecycle', {}).get('preStop', {}),
                  'abort_cannot_restore_withdrawn_EG_exec')
    return restored, []
