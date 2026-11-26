"""Exercise the HPA controller against the fixture's second StatefulSet."""
from copy import deepcopy
from decimal import Decimal
import json
import time
import urllib.error
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


def observe_never_ready(fixture, agent_id):
    client = fixture.client
    name = 'am-startup-witness'
    before = fixture.ownerOf(agentId=agent_id)
    if not before:
        raise RuntimeError('startup_witness_requires_live_owner')
    nodes = client.kjson(['get', 'nodes', '-o', 'json'])['items']
    cpus = [node['status']['allocatable']['cpu'] for node in nodes]
    capacity = max(Decimal(cpu.removesuffix('m')) * (1 if cpu.endswith('m') else 1000) for cpu in cpus)
    template = fixture.templates['b']['spec']['template']
    spec = deepcopy(template['spec'])
    spec['restartPolicy'] = 'Never'
    container, = spec['containers']
    container['env'] += [
        {'name': 'CHRONO_AM_STARTUP_HEALTH_SERVING_ENABLED', 'value': 'true'},
        {'name': 'CHRONO_NATIVE_WARM_POOL_TARGET', 'value': '1'},
        # Hold the real warm-pool startup gate with a separate fixture plan.
        {'name': 'CHRONO_AGENT_MANAGER_SANDBOX_CPU', 'value': str(int(capacity) + 1000) + 'm'},
    ]
    pod = {'apiVersion': 'v1', 'kind': 'Pod', 'spec': spec,
           'metadata': {'name': name, 'namespace': client.namespace}}
    client.apply([pod])
    origin = None
    primary = None
    samples = 0
    try:
        def running():
            current = client.kjson(['get', 'pod', name, '-o', 'json'])
            status = current.get('status', {})
            if status.get('phase') in ('Succeeded', 'Failed'):
                raise RuntimeError('startup_witness_process_failed')
            states = status.get('containerStatuses', [])
            return current if states and states[0].get('state', {}).get('running') is not None else None

        started = client.wait(running, 45, 'startup_witness_process_missing')
        address = started['status']['podIP']
        origin = client.forward(name, 8080)

        def live():
            try:
                return client.http_raw(origin, 'GET', '/livez', timeout=2)[0] == 200
            except (OSError, urllib.error.URLError):
                return False

        client.wait(live, 30, 'startup_witness_liveness_missing')
        for _ in range(5):
            current = running()
            status, reply = client.http(origin, 'GET', '/ready', timeout=3)
            members = fixture.mongo("print(JSON.stringify(d.cma_agent_manager_members.find({},"
                "{relay_address:1}).limit(16).toArray()));")
            has_member = any(urllib.parse.urlsplit(row['relay_address']).hostname == address for row in members)
            owner = fixture.ownerOf(agentId=agent_id)
            if (not current or current['metadata']['uid'] != started['metadata']['uid']
                    or current['status']['containerStatuses'][0].get('ready') is True
                    or status != 503 or not reply or reply.get('reason') != 'startup' or has_member
                    or not owner or owner['am'] not in ('a', 'b') or owner['epoch'] < before['epoch']):
                raise RuntimeError('startup_witness_joined_or_disrupted_serving_members')
            samples += 1
            time.sleep(.5)
    except Exception as error:
        primary = error
    try:
        client.k(['delete', 'pod', name, '--wait=true', '--timeout=30s'], timeout=35)
        if origin:
            child, _ = client.forwards.pop((name, 8080))
            client.stop(child)
    except Exception as cleanup:
        if primary is not None:
            raise ExceptionGroup('startup_witness_execution_and_cleanup', [primary, cleanup]) from None
        raise
    if primary is not None:
        raise primary
    return {'never_ready_samples': samples, 'third_member_registered': False, 'startup_witness_removed': True}
