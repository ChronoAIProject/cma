#!/usr/bin/env python3
"""Check the two local services and report source/image/readiness facts."""
import argparse
import datetime
import json
from pathlib import Path
import subprocess

parser = argparse.ArgumentParser()
parser.add_argument('source_sha')
parser.add_argument('--record', action='store_true', help='owner only: update local deployment evidence')
args = parser.parse_args()
if len(args.source_sha) != 40 or any(c not in '0123456789abcdef' for c in args.source_sha):
    parser.error('expected a full Git SHA')
root = Path(__file__).resolve().parent.parent
k = ['kubectl', '--kubeconfig', str(root / 'private/kubeconfig'),
     '--context', 'kind-cma-scale', '--server', 'https://127.0.0.1:16443',
     '--namespace', 'cma-scale', '--request-timeout=15s']
pods = json.loads(subprocess.check_output(k + ['get', 'pods', '-o', 'json'], text=True))['items']
services = {}
for name, host, path in [('cma-agent-manager', 'am.cma-scale.test', '/ready'),
                         ('cma-event-gateway', 'eg.cma-scale.test', '/readyz')]:
    d = json.loads(subprocess.check_output(k + ['get', 'deployment', name, '-o', 'json'], text=True))
    ps = [p for p in pods if p['metadata'].get('labels', {}).get('app.kubernetes.io/name') == name]
    active = [p for p in ps if not p['metadata'].get('deletionTimestamp')]
    assert len(active) == 2 and len(ps) == 2, 'Expected exactly two nonterminating pods: ' + name
    assert d['spec']['template']['metadata']['annotations']['cma.chronoai.dev/source-sha'] == args.source_sha
    assert all(p['metadata'].get('annotations', {}).get('cma.chronoai.dev/source-sha') == args.source_sha for p in active)
    assert all(any(c['type'] == 'Ready' and c['status'] == 'True' for c in p['status'].get('conditions', [])) for p in active)
    assert d['status'].get('readyReplicas') == d['status'].get('availableReplicas') == 2
    nodes = len({p['spec']['nodeName'] for p in active})
    assert nodes == 2, 'Replicas are co-located: ' + name
    image = d['spec']['template']['spec']['containers'][0]['image']
    image_info = json.loads(subprocess.check_output(['docker', 'image', 'inspect', image], text=True))[0]
    status = subprocess.check_output(['curl', '--silent', '--show-error', '--max-time', '10',
        '--cacert', str(root / 'private/tls.crt'), '--resolve', host + ':18080:127.0.0.1',
        '-o', '/dev/null', '-w', '%{http_code}', 'https://' + host + ':18080' + path], text=True)
    assert status == '200', name + ' TLS readiness failed'
    digests = sorted({c['imageID'] for p in active for c in p['status']['containerStatuses']
                      if c['name'] in ['api', 'cmaeg']})
    services[name] = {'source_sha': args.source_sha, 'image_ref': image,
        'docker_config_id': image_info['Id'], 'runtime_digests': digests,
        'architecture': image_info['Architecture'], 'desired': 2, 'ready': 2,
        'available': 2, 'worker_count': nodes, 'terminating': 0,
        'restarts': sum(c['restartCount'] for p in active for c in p['status']['containerStatuses']),
        'tls_ready_http_status': int(status)}
    print(name + ': 2/2 Ready, two workers, terminating=0, TLS=200; ' + image)
    print('  CRI ' + ','.join(digests))
result = {'event': 'local_rollout', 'observed_at_utc': datetime.datetime.now(datetime.timezone.utc).isoformat(),
          'context': 'kind-cma-scale', 'namespace': 'cma-scale', 'services': services,
          'scope': 'infrastructure readiness only; no user-journey or M3 acceptance'}
if args.record:
    (root / 'evidence/latest-deployment.json').write_text(json.dumps(result, indent=2) + '\n')
    with (root / 'evidence/deployment-ledger.jsonl').open('a') as f:
        f.write(json.dumps(result) + '\n')
