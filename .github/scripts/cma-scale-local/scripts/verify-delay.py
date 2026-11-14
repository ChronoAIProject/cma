#!/usr/bin/env python3
"""Measure the same local proxy route before/during a bounded network delay."""
import json
from pathlib import Path
import ssl
import statistics
import subprocess
import time
import urllib.request

ROOT=Path(__file__).resolve().parent.parent
tls=ssl.create_default_context(cafile=str(ROOT/'private/tls.crt'))
def samples():
    values=[]
    for _ in range(10):
        request=urllib.request.Request('https://127.0.0.1:18080/ready',
            headers={'Host':'am.cma-scale.test','X-Scale-Fault':'delay'})
        start=time.monotonic()
        with urllib.request.urlopen(request,context=tls,timeout=5) as response:
            if response.status!=200: raise SystemExit('Network delay check received non-200')
            response.read()
        values.append((time.monotonic()-start)*1000)
        time.sleep(0.1)
    return values
control=samples()
process=subprocess.Popen([str(ROOT/'scripts/fault.sh'),'delay','am','150','10'],cwd=ROOT,
                         stdout=subprocess.PIPE,stderr=subprocess.PIPE,text=True)
try:
    line=process.stdout.readline()
    if 'Delay active' not in line: raise SystemExit('Delay activation did not succeed')
    delayed=samples()
    process.wait(timeout=20)
    if process.returncode: raise SystemExit('Delay cleanup did not succeed')
finally:
    if process.poll() is None:
        process.terminate()
        process.wait(timeout=10)
after=samples()
evidence={'injected_ms':150,'count_each':10,
          'control_mean_ms':statistics.mean(control),'delayed_mean_ms':statistics.mean(delayed),
          'after_mean_ms':statistics.mean(after),'same_proxy_route':True}
evidence['mean_added_ms']=evidence['delayed_mean_ms']-evidence['control_mean_ms']
(ROOT/'evidence/network-delay.json').write_text(json.dumps(evidence,indent=2)+'\n')
if not (125 < evidence['mean_added_ms'] < 300 and evidence['after_mean_ms'] < 100):
    raise SystemExit('Injected delay or automatic cleanup verification failed')
print(json.dumps(evidence,indent=2))
