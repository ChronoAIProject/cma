#!/usr/bin/env python3
"""Check the local replica set with a narrowly scoped probe credential."""
import json
from pathlib import Path
import subprocess
import time

ROOT=Path(__file__).resolve().parent.parent
K=str(ROOT/'scripts/k')
name='mongo-check-'+str(int(time.time()))
js="""const a=db.getSiblingDB('admin');
a.auth('probe',process.env.PROBE_PASSWORD);
const s=a.runCommand({replSetGetStatus:1});
const primary=s.members.filter(m=>m.state===1).length;
const secondary=s.members.filter(m=>m.state===2).length;
const healthy=s.members.filter(m=>m.health===1).length;
print(JSON.stringify({members:s.members.length,primary,secondary,healthy,ok:s.ok}));
quit(s.ok===1&&primary===1&&secondary===2&&healthy===3?0:1);
"""
job={'apiVersion':'batch/v1','kind':'Job','metadata':{'name':name,'namespace':'cma-scale'},'spec':{
    'activeDeadlineSeconds':60,'ttlSecondsAfterFinished':300,'backoffLimit':0,
    'template':{'spec':{'restartPolicy':'Never','automountServiceAccountToken':False,
    'containers':[{'name':'check','image':'mongo:8.0.13',
    'command':['mongosh','--quiet','--host','mongo-0.mongo.cma-scale.svc.cluster.local','--eval',js],
    'env':[{'name':'PROBE_PASSWORD','valueFrom':{'secretKeyRef':{'name':'mongo-bootstrap','key':'PROBE_PASSWORD'}}}],
    'resources':{'requests':{'cpu':'25m','memory':'64Mi'},'limits':{'cpu':'200m','memory':'256Mi'}}}]}}}}
subprocess.run([K,'create','-f','-'],input=json.dumps(job),text=True,check=True)
try:
    subprocess.run([K,'wait','--for=condition=complete','job/'+name,'--timeout=75s'],check=True)
    result=subprocess.check_output([K,'logs','job/'+name],text=True)
    parsed=json.loads(result)
    (ROOT/'evidence/mongo-health.json').write_text(json.dumps(parsed,indent=2)+'\n')
    print(json.dumps(parsed))
finally:
    subprocess.run([K,'delete','job',name,'--wait=false'],check=False)
