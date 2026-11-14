#!/usr/bin/env python3
"""Record TLS, ready endpoints and hashed backend distribution without bodies."""
import collections
import hashlib
import json
from pathlib import Path
import ssl
import subprocess
import time
import urllib.request

ROOT=Path(__file__).resolve().parent.parent
tls=ssl.create_default_context(cafile=str(ROOT/'private/tls.crt'))
evidence={'source_sha':(ROOT/'evidence/binary-source-sha.txt').read_text().strip(),
          'timestamp_utc':time.strftime('%Y-%m-%dT%H:%M:%SZ',time.gmtime()),'services':{}}
for name,host,path in [('cma-agent-manager','am.cma-scale.test','/ready'),('cma-event-gateway','eg.cma-scale.test','/readyz')]:
    desired=json.loads(subprocess.check_output([str(ROOT/'scripts/k'),'get','deployment',name,'-o','json']))['spec']['replicas']
    backends=collections.Counter(); statuses=collections.Counter()
    for _ in range(20):
        request=urllib.request.Request('https://127.0.0.1:18080'+path,headers={'Host':host})
        with urllib.request.urlopen(request,context=tls,timeout=5) as response:
            statuses[response.status]+=1
            upstream=response.headers.get('X-Scale-Upstream')
            if not upstream: raise SystemExit('Missing serving-backend evidence')
            backends[hashlib.sha256(upstream.encode()).hexdigest()]+=1
        time.sleep(0.1)
    evidence['services'][name]={'desired':desired,'statuses':dict(statuses),'backends_sha256':dict(backends)}
    if len(backends)!=desired or statuses[200]!=20:
        raise SystemExit('Serving-backend coverage failed')
path=ROOT/'evidence/routing.json'
path.write_text(json.dumps(evidence,indent=2)+'\n')
print(json.dumps(evidence,indent=2))
