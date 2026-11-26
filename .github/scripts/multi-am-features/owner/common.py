"""Bounded local WS4 subprocess/HTTP helpers; all raw logs remain private."""
import datetime
import json
import os
from pathlib import Path
import re
import signal
import subprocess
import time
import urllib.error
import urllib.request

def utc():
    return datetime.datetime.now(datetime.timezone.utc).isoformat()

def write(path, value):
    path = Path(path)
    path.write_text(json.dumps(value, indent=2) + '\n')
    path.chmod(0o600)

def require_feature_fixture(fixture):
    ready = fixture.ready
    run_id = ready.get('run_id', '')
    if (ready.get('fixture') != 'features' or fixture.client.namespace != 'cma-native-am'
            or not re.fullmatch(r'cma-ci-[0-9a-f]{12}', run_id) or ready.get('context') != 'kind-' + run_id
            or not re.fullmatch(r'https?://127\.0\.0\.1:[0-9]+', ready.get('server', ''))):
        raise ValueError('disposable_feature_fixture_required')

class NoRedirect(urllib.request.HTTPRedirectHandler):
    def redirect_request(self, *_):
        return None

class Client:
    def __init__(self, ready, output):
        self.ready = ready
        self.output = Path(output)
        self.children = []
        self.forwards = {}
        self.deadline = time.monotonic() + 2400
        self.namespace = ready['namespace']
        self.kbase = ['kubectl',
            '--kubeconfig', ready['kubeconfig_path'], '--context', ready['context'],
            '--server', ready['server'], '--request-timeout=15s', '-n', self.namespace]

    def command(self, argv, data=None, timeout=30, check=True):
        remaining = self.deadline-time.monotonic()
        if remaining <= 0:
            raise TimeoutError('client_deadline')
        child = subprocess.Popen(argv, stdin=subprocess.PIPE, stdout=subprocess.PIPE,
                                 stderr=subprocess.PIPE, text=True, start_new_session=True)
        self.children.append(child)
        try:
            out, err = child.communicate(data, timeout=min(timeout, remaining))
        except BaseException:
            self.stop(child)
            raise
        if len(out)+len(err) > 4*1024*1024:
            raise RuntimeError('subprocess_output_bound')
        if check and child.returncode:
            label = 'command-'+str(time.time_ns())
            (self.output/(label+'.private.log')).write_text(out+'\n'+err)
            raise RuntimeError(label+'_exit_'+str(child.returncode))
        return out if check else (child.returncode, out)

    def k(self, args, **kwargs):
        return self.command(self.kbase+args, **kwargs)

    def kjson(self, args, **kwargs):
        return json.loads(self.k(args, **kwargs))

    def apply(self, objects):
        for obj in objects:
            self.k(['create', '-f', '-', '-o', 'name'], data=json.dumps(obj))

    def wait(self, predicate, seconds, label):
        end = min(self.deadline, time.monotonic()+seconds)
        while time.monotonic() < end:
            value = predicate()
            if value:
                return value
            time.sleep(1)
        raise TimeoutError(label)

    def forward(self, name, remote, renew=False):
        key=(name,remote)
        if key in self.forwards and not renew:
            return self.forwards[key][1]
        local_port=''
        if key in self.forwards:
            local_port=self.forwards[key][1].rsplit(':',1)[1]
            self.stop(self.forwards[key][0])
        path=self.output/('forward-'+name+'-'+str(remote)+'-'+str(time.time_ns())+'.private.log')
        with path.open('w') as log:
            child=subprocess.Popen(self.kbase+['port-forward','--address','127.0.0.1',
                'pod/'+name,local_port+':'+str(remote)],stdout=log,stderr=log,start_new_session=True)
        self.children.append(child)
        def started():
            if child.poll() is not None:
                raise RuntimeError('forward_failed_'+name)
            match=re.search(r'Forwarding from 127\.0\.0\.1:(\d+)',path.read_text())
            return 'http://127.0.0.1:'+match[1] if match else None
        origin=self.wait(started,30,'forward_not_ready_'+name)
        self.forwards[key]=(child,origin)
        return origin

    def http_raw(self, origin, method, path, body=None, headers=None, timeout=15):
        if not origin.startswith('http://127.0.0.1:'):
            raise ValueError('loopback_origin_required')
        raw=json.dumps(body).encode() if body is not None else None
        request=urllib.request.Request(origin+path,data=raw,method=method,
            headers={'Content-Type':'application/json',**(headers or {})})
        try:
            response=urllib.request.build_opener(NoRedirect).open(request,timeout=timeout)
        except urllib.error.HTTPError as exc:
            response=exc
        with response:
            raw=response.read(1024*1024+1)
            if len(raw)>1024*1024:
                raise ValueError('http_output_bound')
            return response.status,response.headers,raw

    def http(self, *args, **kwargs):
        status,_,raw=self.http_raw(*args,**kwargs)
        return status,json.loads(raw) if raw else None

    @staticmethod
    def stop(child):
        # Port forwards and client commands are independent owned process groups.
        try:
            os.killpg(child.pid,signal.SIGTERM)
        except ProcessLookupError:
            pass
        try:
            child.wait(timeout=3)
        except subprocess.TimeoutExpired:
            try:
                os.killpg(child.pid,signal.SIGKILL)
            except ProcessLookupError:
                pass
            child.wait(timeout=3)

    def close(self):
        for child in reversed(self.children):
            if child.poll() is None:
                self.stop(child)
        return all(child.poll() is not None for child in self.children)
