"""Read-only checks for actual Deployment-owned processes, not label counts alone."""
import rules as g


def controlled_by(obj, uid):
    return any(x.get('uid') == uid and x.get('controller') is True
               for x in obj['metadata'].get('ownerReferences', []))


def cohort(obj):
    sets = g.command(g.K + ['get', 'replicasets', '-o', 'json'])['items']
    owned = {r['metadata']['uid']: r for r in sets
             if controlled_by(r, obj['metadata']['uid'])}
    labels = obj['spec']['selector']['matchLabels']
    pods = g.command(g.K + ['get', 'pods', '-o', 'json'])['items']
    selected = [p for p in pods if
                all(p['metadata'].get('labels', {}).get(k) == v for k, v in labels.items())
                or any(controlled_by(p, uid) for uid in owned)]
    g.require(all(any(controlled_by(p, uid) for uid in owned) for p in selected),
              'foreign_pod_matches_service_selector')
    return owned, selected


def ready(pod):
    return not pod['metadata'].get('deletionTimestamp') and any(
        c.get('type') == 'Ready' and c.get('status') == 'True'
        for c in pod.get('status', {}).get('conditions', []))


def original_process(obj, template_sha256=None):
    """Project only safe identities of the one serving pre-pause incarnation."""
    owned, pods = cohort(obj)
    g.require(len(pods) == 1 and ready(pods[0]), 'original_legacy_process_required')
    pod = pods[0]
    sets = [rs for uid, rs in owned.items() if controlled_by(pod, uid)]
    template_sha256 = template_sha256 or g.template_identity(obj['spec']['template'])
    g.require(len(sets) == 1 and not sets[0]['metadata'].get('deletionTimestamp') and
              g.template_identity(sets[0]['spec']['template']) == template_sha256,
              'original_legacy_template_required')
    names = [c['name'] for c in sets[0]['spec']['template']['spec']['containers']]
    statuses = pod.get('status', {}).get('containerStatuses', [])
    g.require(names and len(set(names)) == len(names) and
              sorted(s['name'] for s in statuses) == sorted(names) and
              sorted(c['name'] for c in pod['spec']['containers']) == sorted(names),
              'original_container_status_required')
    containers = []
    for status in sorted(statuses, key=lambda s: s['name']):
        running = status.get('state', {}).get('running', {})
        fields = (status.get('containerID'), status.get('imageID'), running.get('startedAt'))
        g.require(status.get('ready') is True and all(isinstance(v, str) and v for v in fields)
                  and type(status.get('restartCount')) is int and status['restartCount'] >= 0,
                  'original_container_incarnation_required')
        containers.append(dict(name=status['name'], container_id=fields[0], image_id=fields[1],
                               started_at=fields[2], restart_count=status['restartCount']))
    return dict(deployment_uid=obj['metadata']['uid'], rs_uid=sets[0]['metadata']['uid'],
                pod_uid=pod['metadata']['uid'], template_sha256=template_sha256,
                containers=containers)


def same_original_process(obj, saved):
    g.require(isinstance(saved, dict) and saved.get('template_sha256'),
              'saved_original_process_required')
    g.require(obj['metadata']['uid'] == saved.get('deployment_uid'),
              'original_deployment_changed')
    g.require(original_process(obj, saved['template_sha256']) == saved,
              'original_process_changed')


def stable(obj, count, current_template=True, distinct=False):
    spec, status = obj['spec'], obj.get('status', {})
    g.require(spec.get('replicas') == count and
              status.get('observedGeneration') == obj['metadata']['generation'] and
              all(status.get(k, 0) == count for k in
                  ('replicas', 'readyReplicas', 'availableReplicas')),
              'deployment_not_fully_converged')
    owned, pods = cohort(obj)
    g.require(len(pods) == count and all(ready(p) for p in pods),
              'wait_for_actual_termination_and_ready_owners')
    if current_template:
        g.require(not spec.get('paused', False) and
                  status.get('updatedReplicas', 0) == count, 'candidate_not_fully_converged')
        exact = [uid for uid, rs in owned.items() if
                 g.template_identity(rs['spec']['template']) ==
                 g.template_identity(spec['template'])]
        g.require(len(exact) == 1 and all(controlled_by(p, exact[0]) for p in pods),
                  'pods_do_not_match_current_controller_template')
        for pod in pods:
            for container in spec['template']['spec']['containers']:
                if container['name'] not in g.TARGETS.values():
                    continue
                statuses = [s for s in pod.get('status', {}).get('containerStatuses', [])
                            if s['name'] == container['name']]
                actual = [c for c in pod['spec']['containers'] if c['name'] == container['name']]
                digest = container['image'].split('@')[-1]
                g.require(len(actual) == len(statuses) == 1 and digest.startswith('sha256:')
                          and actual[0]['image'] == container['image']
                          and statuses[0].get('imageID', '').split('@')[-1] == digest
                          and statuses[0].get('ready') is True, 'running_image_mismatch')
    if distinct:
        nodes = {p['spec'].get('nodeName') for p in pods}
        g.require(None not in nodes and '' not in nodes and len(nodes) == count,
                  'actual_distinct_host_placement_required')
    return pods
