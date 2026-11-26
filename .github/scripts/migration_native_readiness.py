"""Strict projected readiness and disposable probe manifests, without secrets."""
import re

from migration_kind_manifests import NAMESPACE
from migration_native_process import require


def image_id_matches(value, expected):
    if not isinstance(value, str): return False
    value = re.sub(r'^(?:containerd|docker-pullable)://', '', value)
    return value in (expected['config'], expected['image'], expected['image'].split('@')[1])


def ready_pod(pod, expected, container_name='api', namespace=NAMESPACE):
    metadata = pod.get('metadata', {})
    require(metadata.get('namespace') == namespace and metadata.get('name') and metadata.get('uid')
            and not metadata.get('deletionTimestamp'), 'native_ready_pod_identity_invalid')
    status = pod.get('status', {})
    require(status.get('phase') == 'Running' and any(c.get('type') == 'Ready' and c.get('status') == 'True'
            for c in status.get('conditions', [])), 'native_pod_not_ready')
    containers = [c for c in pod.get('spec', {}).get('containers', []) if c.get('name') == container_name]
    statuses = [c for c in status.get('containerStatuses', []) if c.get('name') == container_name]
    require(len(containers) == len(statuses) == 1, 'native_ready_container_missing')
    current = statuses[0]
    require(containers[0].get('image') == expected['image'] and current.get('ready') is True
            and type(current.get('restartCount')) is int and current['restartCount'] == 0
            and image_id_matches(current.get('imageID'), expected)
            and re.fullmatch('containerd://[a-f0-9]{64}', current.get('containerID', ''))
            and current.get('state', {}).get('running', {}).get('startedAt'), 'native_container_identity_invalid')
    return {'name': metadata['name'], 'uid': metadata['uid'], 'namespace': namespace,
            'image': expected['image'], 'image_id': current['imageID'], 'restart_count': 0,
            'container_id': current['containerID'], 'started_at': current['state']['running']['startedAt']}


def select_api(document, expected):
    require(isinstance(document.get('items'), list) and not document.get('metadata', {}).get('continue'),
            'native_api_list_incomplete')
    live = [p for p in document['items'] if not p.get('metadata', {}).get('deletionTimestamp')]
    require(len(live) == 1, 'native_single_reader_required')
    return ready_pod(live[0], expected)


def job(name, image, args, *, code=None, env=None):
    require(re.fullmatch('[a-z0-9-]{1,63}', name), 'native_job_name_invalid')
    container = {'name': name, 'image': image, 'imagePullPolicy': 'Never', 'args': args,
                 'resources': {'requests': {'cpu': '50m', 'memory': '128Mi'},
                               'limits': {'cpu': '1', 'memory': '1Gi'}}}
    if code is not None: container.update(command=['/usr/local/bin/python3', '-B', '-c', code], args=[])
    if env is not None: container['env'] = [{'name': k, 'value': v} for k, v in env.items()]
    return {'apiVersion': 'batch/v1', 'kind': 'Job', 'metadata': {'name': name, 'namespace': NAMESPACE},
            'spec': {'backoffLimit': 0, 'activeDeadlineSeconds': 120, 'template': {'spec': {
                'restartPolicy': 'Never', 'automountServiceAccountToken': False, 'containers': [container]}}}}


TLS_PROBE = '''import ipaddress,json,os,socket,ssl,time,urllib.request
class NoRedirect(urllib.request.HTTPRedirectHandler):
    def redirect_request(self,*args,**kwargs): return None
context=ssl.create_default_context()
context.minimum_version=ssl.TLSVersion.TLSv1_2
opener=urllib.request.build_opener(urllib.request.ProxyHandler({}),NoRedirect,
    urllib.request.HTTPSHandler(context=context))
expected={'api.localhost.direct':os.environ['NATIVE_API_SERVICE_IP'],
          'provider.localhost.direct':os.environ['NATIVE_PROVIDER_SERVICE_IP']}
for value in expected.values(): ipaddress.ip_address(value)
for attempt in range(45):
    try:
        resolved=all({row[4][0] for row in socket.getaddrinfo(host,None)}=={address}
                     for host,address in expected.items())
    except socket.gaierror: resolved=False
    if resolved: break
    if attempt==44: raise RuntimeError('native_dns_reload_unobserved')
    time.sleep(2)
rows=[]
for url in ('https://api.localhost.direct/livez','https://provider.localhost.direct:8080/health'):
    request=urllib.request.Request(url,headers={'Connection':'close'})
    with opener.open(request,timeout=15) as response:
        if response.status!=200 or response.url!=url: raise RuntimeError('native_probe_response_invalid')
        body=response.read(4097)
        if len(body)>4096: raise RuntimeError('native_probe_body_bound')
        if url.endswith('/health') and json.loads(body).get('protocol')!='openai_responses_sse':
            raise RuntimeError('native_provider_probe_invalid')
    rows.append({'url':url,'status':200,'verified_tls':True})
print(json.dumps({'schema':'migration-native-tls-probe-v1','checks':rows}))
'''


def projected_boot_list(value):
    """Retain failed startup states without Pod environment or termination text."""
    rows = []
    for pod in value.get('items', []):
        meta = pod.get('metadata', {});status = pod.get('status', {})
        containers = []
        for item in status.get('containerStatuses', []):
            states = {}
            for key in ('state', 'lastState'):
                states[key] = {kind: {name: field for name, field in fields.items()
                                     if name in ('reason', 'exitCode', 'startedAt', 'finishedAt', 'signal')}
                               for kind, fields in item.get(key, {}).items()}
            containers.append({**{key: item.get(key) for key in
                ('name', 'ready', 'restartCount', 'imageID', 'containerID')}, **states})
        rows.append({'metadata': {key: meta.get(key) for key in ('name', 'uid', 'resourceVersion')},
                     'spec': {'containers': [{k: c.get(k) for k in ('name', 'image')}
                                             for c in pod.get('spec', {}).get('containers', [])]},
                     'status': {'phase': status.get('phase'), 'containerStatuses': containers,
                                'conditions': [{k: c.get(k) for k in ('type', 'status')}
                                               for c in status.get('conditions', [])]}})
    return {'metadata': {'resourceVersion': value.get('metadata', {}).get('resourceVersion')}, 'items': rows}
