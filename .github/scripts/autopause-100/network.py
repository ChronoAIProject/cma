"""Route public TLS fixture names inside this exclusively owned Kind cluster."""
import hashlib
import json
import re
import time


ROUTES = [('api.localhost.direct', 'api-proxy', 443, '/ready', 200),
          ('provider.localhost.direct', 'provider', 8080, '/health', 200),
          ('storage.localhost.direct', 'storage', 443, '/health', 401)]


def configure_dns(client, ready, output):
    namespace = ready['namespace']
    run_id = ready['run_id']
    if (not ready['context'].startswith('kind-cma-autopause18-')
            or not ready['server'].startswith('https://127.0.0.1:')
            or not re.fullmatch(r'[a-z0-9-]+', namespace)
            or not re.fullmatch(r'[a-z0-9-]+', run_id)):
        raise ValueError('owned_local_dns_required')
    ns = client.kjson(['get', 'namespace', namespace, '-o', 'json'])
    if ns['metadata'].get('labels', {}).get('cma.chronoai.dev/run') != run_id:
        raise ValueError('dns_namespace_ownership_mismatch')
    current = client.kjson(['-n', 'kube-system', 'get', 'configmap', 'coredns', '-o', 'json'])
    before = current['data']['Corefile']
    rules = '\n'.join('    rewrite name exact ' + host + ' ' + service + '.' + namespace
                      + '.svc.cluster.local' for host, service, *_ in ROUTES)
    marker = '\n    # autopause-100 ' + run_id + '\n' + rules + '\n'
    if marker not in before:
        if before.count('.:53 {') != 1 or any(host in before for host, *_ in ROUTES):
            raise ValueError('unexpected_existing_fixture_dns_rules')
        after = before.replace('.:53 {', '.:53 {' + marker, 1)
        patch = {'metadata': {'resourceVersion': current['metadata']['resourceVersion']},
                 'data': {'Corefile': after}}
        client.k(['-n', 'kube-system', 'patch', 'configmap', 'coredns',
                  '--type=merge', '-p', json.dumps(patch)])
        client.k(['-n', 'kube-system', 'rollout', 'restart', 'deployment/coredns'])
        client.k(['-n', 'kube-system', 'rollout', 'status', 'deployment/coredns',
                  '--timeout=90s'], timeout=100)
    else:
        after = before
    actual = client.kjson(['-n', 'kube-system', 'get', 'configmap', 'coredns', '-o', 'json'])
    if actual['data']['Corefile'] != after:
        raise ValueError('fixture_dns_readback_mismatch')
    (output / 'dns-before.txt').write_text(before)
    (output / 'dns-after.txt').write_text(after)
    return {'namespace_uid': ns['metadata']['uid'],
            'corefile_sha256': hashlib.sha256(after.encode()).hexdigest()}


def prove_routes(client, ready, output):
    routes = []
    for host, service, port, path, expected_status in ROUTES:
        address = client.kjson(['get', 'service', service, '-o', 'json'])['spec']['clusterIP']
        routes.append({'host': host, 'address': address, 'port': port, 'path': path,
                       'expected_status': expected_status})
    script = '''import json,socket,urllib.request,urllib.error
routes = json.loads(ROUTES)
for route in routes:
    addresses = sorted({value[4][0] for value in socket.getaddrinfo(
        route['host'], route['port'], type=socket.SOCK_STREAM)})
    assert addresses == [route['address']], 'fixture_dns_address_mismatch'
    url = 'https://' + route['host'] + ':' + str(route['port']) + route['path']
    try:
        with urllib.request.urlopen(url, timeout=10) as response:
            status = response.status
    except urllib.error.HTTPError as error:
        status = error.code
    assert status == route['expected_status'], 'fixture_tls_route_status_mismatch'
    print(json.dumps({'host': route['host'], 'addresses': addresses,
                      'status': status, 'tls_verified': True}), flush=True)
'''.replace('ROUTES', repr(json.dumps(routes)), 1)
    name = 'autopause-route-proof-' + str(time.time_ns())
    job = {'apiVersion': 'batch/v1', 'kind': 'Job',
           'metadata': {'name': name, 'namespace': ready['namespace'],
                        'labels': {'qualification': ready['run_id']}},
           'spec': {'activeDeadlineSeconds': 45, 'backoffLimit': 0,
                    'template': {'spec': {'restartPolicy': 'Never',
                        'automountServiceAccountToken': False,
                        'containers': [{'name': 'probe', 'image': ready['runtime_images']['base'],
                            'imagePullPolicy': 'Never', 'command': ['/usr/bin/python3', '-B', '-c', script],
                            'resources': {'requests': {'cpu': '25m', 'memory': '32Mi'},
                                          'limits': {'cpu': '250m', 'memory': '128Mi'}}}]}}}}
    client.apply([job])
    def complete():
        status = client.kjson(['get', 'job', name, '-o', 'json']).get('status', {})
        if status.get('failed'):
            raise RuntimeError('runtime_visible_route_proof_failed')
        return status.get('succeeded') == 1
    try:
        client.wait(complete, 60, 'runtime_visible_route_proof_timeout')
    finally:
        code, logs = client.k(['logs', 'job/' + name, '--limit-bytes=16384'], check=False)
        (output / 'dns-tls-proof.log').write_text(logs)
        client.k(['delete', 'job', name, '--wait=true', '--timeout=20s'], timeout=25)
    if code:
        raise RuntimeError('runtime_visible_route_proof_log_unavailable')
    return {'routes': [json.loads(line) for line in logs.splitlines()], 'operator_pod_exec': False}
