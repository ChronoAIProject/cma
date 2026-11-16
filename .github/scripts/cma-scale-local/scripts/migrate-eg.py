#!/usr/bin/env python3
"""Initialize only the disposable local EG database through the application CLI."""
import json
from pathlib import Path
import subprocess
import time
ROOT=Path(__file__).resolve().parent.parent
K=str(ROOT/'scripts/k')
sha=(ROOT/'evidence/binary-source-sha.txt').read_text().strip()
name='eg-init-'+str(int(time.time()))
commands=['--migrate','--migrate-thread-fences','--migrate-worker-credentials','--migrate-whatsapp-forms','--migrate-media-continuations']
job={'apiVersion':'batch/v1','kind':'Job','metadata':{'name':name,'namespace':'cma-scale'},'spec':{
    'activeDeadlineSeconds':180,'ttlSecondsAfterFinished':300,'backoffLimit':0,
    'template':{'spec':{'restartPolicy':'Never','automountServiceAccountToken':False,
    'securityContext':{'runAsNonRoot':True,'runAsUser':10001,'runAsGroup':10001,'seccompProfile':{'type':'RuntimeDefault'}},
    'containers':[{'name':'migrate','image':'cma-scale-api:'+sha,'imagePullPolicy':'Never',
    'command':['/bin/sh','-ec','; '.join('/usr/local/bin/cmaeg '+c for c in commands)],
    'envFrom':[{'configMapRef':{'name':'eg-local'}},{'secretRef':{'name':'eg-local'}}],
    'resources':{'requests':{'cpu':'100m','memory':'128Mi'},'limits':{'cpu':'500m','memory':'384Mi'}},
    'securityContext':{'allowPrivilegeEscalation':False,'readOnlyRootFilesystem':True,'capabilities':{'drop':['ALL']}}}]}}}}
subprocess.run([K,'create','-f','-'],input=json.dumps(job),text=True,check=True)
try:
    subprocess.run([K,'wait','--for=condition=complete','job/'+name,'--timeout=190s'],check=True)
    (ROOT/'evidence/eg-migrations.json').write_text(json.dumps({'source':sha,'commands':commands,'passed':True},indent=2)+'\n')
finally:
    subprocess.run([K,'delete','job',name,'--wait=false'],check=False)
