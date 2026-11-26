"""Exercise the HPA controller against the fixture's second StatefulSet."""
import json
import urllib.parse


def observe_scale(fixture):
    client = fixture.client
    name = 'two-am-readiness'
    target = 'am-b'
    baseline = fixture.topology()['ams']
    if not all(baseline[am]['ready'] for am in ('a', 'b')):
        raise RuntimeError('hpa_requires_two_ready_processes')
    peers = {baseline[am]['pod_uid'] for am in ('a', 'b')}
    expected_image = fixture.ready['expected_api_image']
    hpa = {
        'apiVersion': 'autoscaling/v2', 'kind': 'HorizontalPodAutoscaler',
        'metadata': {'name': name, 'namespace': client.namespace},
        'spec': {
            'scaleTargetRef': {'apiVersion': 'apps/v1', 'kind': 'StatefulSet', 'name': target},
            'minReplicas': 1, 'maxReplicas': 1,
            'metrics': [{'type': 'Resource', 'resource': {
                'name': 'cpu', 'target': {'type': 'Utilization', 'averageUtilization': 60}}}],
            'behavior': {'scaleDown': {'stabilizationWindowSeconds': 0}},
        },
    }

    def ready_pods():
        pods = client.kjson(['get', 'pods', '-l', 'ws4-public-am=true', '-o', 'json'])['items']
        return [pod for pod in pods if not pod['metadata'].get('deletionTimestamp') and any(
            row['type'] == 'Ready' and row['status'] == 'True'
            for row in pod.get('status', {}).get('conditions', []))], len(pods)

    def live_members():
        rows = fixture.mongo("print(JSON.stringify(d.cma_agent_manager_members.find("
            "{draining:false,$expr:{$gt:['$expires_at','$$NOW']}},{relay_address:1}).limit(4).toArray()));")
        return {urllib.parse.urlsplit(row['relay_address']).hostname for row in rows}

    def observation(replicas, requires_rescale):
        current = client.kjson(['get', 'hpa', name, '-o', 'json'])
        bounds = current['spec']
        if bounds['minReplicas'] != replicas or bounds['maxReplicas'] != replicas:
            return None
        status = current.get('status', {})
        if requires_rescale and (
            status.get('currentReplicas') != replicas or status.get('desiredReplicas') != replicas
        ):
            return None
        pods, total = ready_pods()
        if len(pods) != replicas + 1 or total != replicas + 1:
            return None
        if not peers <= {pod['metadata']['uid'] for pod in pods}:
            raise RuntimeError('hpa_replaced_a_baseline_manager')
        for pod in pods:
            container, = pod['spec']['containers']
            if container['image'] != expected_image:
                raise RuntimeError('hpa_manager_image_mismatch')
        addresses = {pod['status']['podIP'] for pod in pods}
        return pods if live_members() == addresses else None

    client.apply([hpa])
    complete = []
    forwarded = None
    primary = None
    try:
        for index, replicas in enumerate((1, 2, 1)):
            bounds = {'spec': {'minReplicas': replicas, 'maxReplicas': replicas}}
            client.k(['patch', 'hpa', name, '--type=merge', '-p', json.dumps(bounds)])
            pods = client.wait(lambda: observation(replicas, index > 0), 65, 'hpa_ready_replica_count_not_reached')
            if replicas == 2:
                third, = [pod for pod in pods if pod['metadata']['uid'] not in peers]
                pod_name = third['metadata']['name']
                origin = client.forward(pod_name, 8080)
                forwarded = (pod_name, 8080)
                if client.http_raw(origin, 'GET', '/ready', timeout=3)[0] != 200:
                    raise RuntimeError('hpa_new_manager_readiness_failed')
            complete.append(len(pods))
    except Exception as error:
        primary = error
    try:
        client.k(['delete', 'hpa', name, '--wait=true', '--timeout=15s'], timeout=20)
        if complete != [2, 3, 2]:
            client.k(['scale', 'statefulset/' + target, '--replicas=1'])
        if forwarded:
            child, _ = client.forwards.pop(forwarded)
            client.stop(child)
    except Exception as cleanup:
        if primary is not None:
            raise ExceptionGroup('hpa_execution_and_cleanup', [primary, cleanup]) from None
        raise
    if primary is not None:
        raise primary
    return {'ready_managers': complete, 'controller': 'HorizontalPodAutoscaler', 'trigger': 'replica_bounds'}
