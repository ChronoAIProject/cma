"""Explicit EG-only topology policy plugged into the shared approval/CAS engine."""
import copy
import math
import re

import checkpoints
import hpa_exercise as exercise
import rules as g

NAMES = {'am': 'cma-agent-manager', 'eg': 'cma-event-gateway'}
PREFIX = 's3-eg-only'
ACTIONS = {'quota-raise', 'quota-restore', 'create', 'lower-target',
           'restore-target', 'freeze', 'delete'}
RECOVERY = {'restore-target', 'freeze', 'delete'}


def evidence(value):
    g.require(isinstance(value, str) and re.fullmatch('[0-9a-f]{64}', value) is not None,
              'EG_only_evidence_digest_required')


def qualification(release):
    """Attestations are source-bound and covered by the exact approval context."""
    packet = release.get('eg_only_hpa', {})
    s2 = packet.get('s2', {})
    g.require(s2.get('state') == 'PASS' and s2.get('source') == release['source']
              and s2.get('images') == release['images'], 'EG_only_source_bound_S2_PASS_required')
    evidence(s2.get('evidence_sha256'))
    end_age, start_age = exercise.age(s2.get('completed_at')), exercise.age(s2.get('started_at'))
    g.require(0 <= end_age and start_age - end_age >= 1800, 'EG_only_completed_S2_soak_required')
    timing = release.get('hpa', {}).get('eg', {})
    p95, lifetime = timing.get('handoff_p95_seconds'), timing.get('hard_lifetime_seconds')
    g.require(all(type(v) in (int, float) and math.isfinite(v) for v in (p95, lifetime))
              and 0 <= p95 <= 90 and lifetime >= 760, 'EG_only_p95_and_actual_lifetime_required')
    overlap = 5 + math.ceil(lifetime / 120)
    count = timing.get('capacity_processes_per_service')
    g.require(type(count) is int and count >= overlap and timing.get('cpu_signal_qualified') is True,
              'EG_only_overlap_and_CPU_qualification_required')
    capacity = packet.get('capacity', {})
    evidence(capacity.get('evidence_sha256'))
    headroom = capacity.get('mongo_headroom_percent')
    g.require(capacity.get('topology') == 'am1-off-eg2-on'
              and 0 <= exercise.age(capacity.get('observed_at')) <= 120
              and type(capacity.get('eg_processes')) is int and capacity['eg_processes'] >= overlap
              and type(capacity.get('am_processes')) is int and capacity['am_processes'] == 1
              and type(headroom) in (int, float) and math.isfinite(headroom) and 30 <= headroom <= 100
              and capacity.get('quota_cpu') == g.QUOTA_AFTER
              and all(capacity.get(k) is True for k in
                      ('other_consumers_included', 'all_live_terminating_clients_included',
                       'node_placement_qualified', 'provider_budgets_qualified',
                       'survivor_qualified', 'early_drain_qualified', 'quota_qualified')),
              'EG_only_fresh_complete_topology_capacity_required')
    return s2


def container(spec, role):
    rows = [c for c in spec.get('containers', []) if c.get('name') == g.TARGETS[NAMES[role]]]
    g.require(len(rows) == 1, 'EG_only_unique_container_required')
    return rows[0]


def mode(spec, role):
    flag = 'CHRONO_AGENT_MANAGER_REPLICAS_ENABLED' if role == 'am' else 'CMAEG_REPLICAS_ENABLED'
    rows = [e for e in container(spec, role).get('env', []) if e.get('name') == flag]
    # Literal env overrides envFrom; absence cannot prove OFF without reading secrets.
    g.require(rows == [{'name': flag, 'value': 'false' if role == 'am' else 'true'}],
              'EG_only_explicit_AM_OFF_EG_ON_required')


def cohort_binding(pods):
    identities = []
    for pod in pods:
        statuses = pod.get('status', {}).get('containerStatuses', [])
        names = [c['name'] for c in pod['spec']['containers']]
        g.require(names and len(names) == len(set(names)) and
                  sorted(s['name'] for s in statuses) == sorted(names),
                  'EG_only_complete_container_incarnations_required')
        rows = []
        for status in sorted(statuses, key=lambda s: s['name']):
            values = [status.get('containerID'), status.get('imageID'),
                      status.get('state', {}).get('running', {}).get('startedAt')]
            g.require(status.get('ready') is True and all(isinstance(v, str) and v for v in values)
                      and type(status.get('restartCount')) is int and status['restartCount'] >= 0,
                      'EG_only_running_container_incarnation_required')
            rows.append([status['name'], *values, status['restartCount']])
        identities.append([pod['metadata']['uid'], pod['metadata'].get('ownerReferences'), rows])
    return g.hash_json(sorted(identities, key=lambda row: row[0]))


def topology(release, quota_restoring=False):
    s2 = qualification(release)
    g.require(g.get('HorizontalPodAutoscaler', NAMES['am']) is None, 'EG_only_AM_HPA_prohibited')
    bindings, cohorts = {}, {}
    for role, count in (('am', 1), ('eg', 2)):
        obj = g.get('Deployment', NAMES[role])
        g.require(obj is not None and type(obj['metadata'].get('generation')) is int
                  and obj['metadata']['generation'] > 0, 'EG_only_Deployment_generation_required')
        saved = s2.get('deployments', {}).get(role)
        if quota_restoring:
            # An HPA cycle advances generation even after the exact fixed spec returns.
            # Bind the current generation below; historical UID/spec must still match.
            g.require(isinstance(saved, dict) and all(g.binding(obj)[key] == saved.get(key)
                      for key in ('uid_sha256', 'spec_sha256')), 'EG_only_S2_identity_or_spec_drift')
        else:
            g.same(obj, saved)
        pods = checkpoints.stable(obj, count, distinct=role == 'eg')
        spec = obj['spec']['template']['spec']
        g.require(container(spec, role).get('image') == release['images'][role],
                  'EG_only_selected_image_required')
        for actual in [spec, *(p['spec'] for p in pods)]:
            mode(actual, role)
            if role == 'eg':
                g.require(actual.get('terminationGracePeriodSeconds') == 760
                          and g.cpu(container(actual, role).get('resources', {}).get('requests', {}).get('cpu'))
                          == g.cpu('250m'), 'EG_only_actual_grace_and_CPU_request_required')
        bindings[role] = {'deployment': g.binding(obj), 'cohort_sha256': cohort_binding(pods)}
        cohorts[role] = pods
    pdb = g.get('PodDisruptionBudget', NAMES['eg'])
    g.require(pdb is not None and pdb['spec'].get('minAvailable') == 1
              and pdb['spec'].get('selector') == {'matchLabels': {'app.kubernetes.io/name': NAMES['eg']}}
              and type(pdb['metadata'].get('generation')) is int
              and pdb.get('status', {}).get('observedGeneration') == pdb['metadata']['generation']
              and pdb['status'].get('currentHealthy', 0) >= 2
              and pdb['status'].get('disruptionsAllowed', 0) >= 1,
              'EG_only_current_PDB_required')
    bindings['pdb'] = g.binding(pdb)
    return bindings, cohorts


def exercise_topology(release):
    bindings, cohorts = topology(release)
    for pod in cohorts['eg']:
        ready = next(c for c in pod['status']['conditions'] if c.get('type') == 'Ready')
        g.require(exercise.age(ready.get('lastTransitionTime')) >= 600,
                  'EG_only_ten_minutes_ready_required')
    return bindings, cohorts


def owned(created, release):
    g.require(created is not None, 'EG_only_owned_result_required')
    result = g.load(created)
    prior = result.get('operation', {})
    g.require(result.get('state') == 'APPLIED' and result.get('source') == release['source']
              and prior.get('eg_only') in ACTIONS
              and prior.get('kind') == 'HorizontalPodAutoscaler' and prior.get('name') == NAMES['eg'],
              'EG_only_confirmed_source_bound_owned_HPA_required')
    obj = g.get('HorizontalPodAutoscaler', NAMES['eg'])
    g.same(obj, result.get('after'))
    return obj, prior


def build(action, release, created, hpa_builder, quota_builder):
    g.require(action in ACTIONS, 'EG_only_unknown_action')
    if action.startswith('quota-'):
        if action == 'quota-restore':
            g.require(created is not None, 'EG_only_quota_raise_result_required')
            saved = g.load(created)
            g.require(saved.get('source') == release['source'] and
                      saved.get('operation', {}).get('eg_only') == 'quota-raise',
                      'EG_only_owned_quota_raise_required')
        op = quota_builder(action.removeprefix('quota-'), created)
    elif action == 'create':
        op = hpa_builder('eg-create', release, None)
        op['id'] = PREFIX + '-create'
    else:
        obj, prior = owned(created, release)
        if action in ('lower-target', 'restore-target'):
            g.require(prior.get('eg_only') == ('create' if action == 'lower-target' else 'lower-target'),
                      'EG_only_target_receipt_chain_required')
            op = exercise.build('eg', action, release, created, exercise_topology, PREFIX)
        else:
            g.require(prior.get('eg_only') in ({'freeze'} if action == 'delete' else
                                              {'create', 'lower-target', 'restore-target'}),
                      'EG_only_freeze_delete_receipt_chain_required')
            op = dict(id=PREFIX + '-' + action, stage='S3', context=g.CONTEXT,
                      namespace=g.NAMESPACE, kind='HorizontalPodAutoscaler', name=NAMES['eg'],
                      action='delete' if action == 'delete' else 'patch', expected_before=g.binding(obj))
            if action == 'freeze':
                spec = copy.deepcopy(obj['spec'])
                for direction in ('scaleUp', 'scaleDown'):
                    spec['behavior'][direction]['selectPolicy'] = 'Disabled'
                op['changes'] = {'spec': spec}
    op['eg_only'] = action
    if action not in RECOVERY:
        bindings, _ = topology(release, quota_restoring=action == 'quota-restore')
        op['topology_bindings'] = bindings
    return op


def checkpoint(obj, op, release, quota_checkpoint):
    action = op['eg_only']
    g.require(action in ACTIONS and op['stage'] == 'S3' and
              (op['kind'] == 'ResourceQuota' and action.startswith('quota-') and op['name'] == 'sandbox-quota'
               or op['kind'] == 'HorizontalPodAutoscaler' and not action.startswith('quota-')
               and op['name'] == NAMES['eg']), 'EG_only_operation_scope')
    if action in RECOVERY:
        g.same(obj, op['expected_before'])
        # Incident recovery depends on owned identity/spec, never on healthy telemetry.
        return
    bindings, cohorts = topology(release, quota_restoring=action == 'quota-restore')
    g.require(bindings == op['topology_bindings'], 'EG_only_topology_changed_after_plan')
    quota = obj if op['kind'] == 'ResourceQuota' else g.get('ResourceQuota', 'sandbox-quota')
    quota_checkpoint(quota, action == 'quota-restore')
    if action.startswith('quota-'):
        g.require(g.get('HorizontalPodAutoscaler', NAMES['eg']) is None,
                  'EG_only_quota_change_requires_no_HPAs')
    else:
        g.require(g.quota_cpu(quota['spec'], g.QUOTA_AFTER), 'EG_only_raised_quota_required')
        exercise.fresh_metrics(NAMES['eg'], cohorts['eg'])
        if action == 'lower-target':
            exercise.checkpoint(obj, op, release, exercise_topology)
