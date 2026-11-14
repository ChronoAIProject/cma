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
