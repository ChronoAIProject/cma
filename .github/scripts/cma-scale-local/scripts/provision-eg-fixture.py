#!/usr/bin/env python3
"""Provision isolated local EG fixture users without exposing credentials."""
import json
import argparse
import os
from pathlib import Path
import secrets
import re
import socket
import subprocess
import time
import uuid

ROOT = Path(__file__).resolve().parent.parent
OPTIONS = argparse.ArgumentParser()
OPTIONS.add_argument('--cohort', choices=['off', 'on'], required=True)
OPTIONS.add_argument('--run-name')
options = OPTIONS.parse_args()
COHORT = options.cohort
RUN_NAME = options.run_name or COHORT
if not re.fullmatch('[a-z0-9][a-z0-9-]{0,31}', RUN_NAME):
    OPTIONS.error('run-name must be a short lowercase local resource name')
PRIVATE = ROOT / ('private/eg-fixture-' + RUN_NAME)
K = ['kubectl', '--kubeconfig', str(ROOT / 'private/kubeconfig'),
     '--context', 'kind-cma-scale', '--server', 'https://127.0.0.1:16443',
     '--namespace', 'cma-scale', '--request-timeout=15s']


def private_json(path, data):
    fd = os.open(path, os.O_WRONLY | os.O_CREAT | os.O_EXCL, 0o600)
    with os.fdopen(fd, 'w') as output:
        json.dump(data, output, indent=2)
        output.write('\n')


def kube(*args, data=None):
    p = subprocess.run(K + list(args), input=json.dumps(data) if data else None,
                       text=True, capture_output=True, timeout=150)
    if p.returncode:
        raise RuntimeError('Local Kubernetes operation failed: ' + ' '.join(args[:2]))
    return p.stdout


def role(database, migration=False):
    arg = '--print-migration-role' if migration else '--print-runtime-role'
    p = subprocess.run([str(ROOT / 'bin/cmaeg-fixture-tools'), arg],
                       env={**os.environ, 'CMAEG_MONGODB_DATABASE': database},
                       text=True, capture_output=True, check=True)
    return json.loads(p.stdout)


def main():
    PRIVATE.mkdir(mode=0o700, exist_ok=False)
    database = 'eg_diag_' + uuid.uuid4().hex
    credentials = {user: secrets.token_hex(32)
                   for user in ['migrator', 'gateway', 'oracle']}
    credentials['database'] = database
    private_json(PRIVATE / 'credentials.json', credentials)
    runtime = role(database)
    migration = role(database, True)
    migration['privileges'].append({'resource': {'db': database, 'collection': ''},
                                    'actions': ['listCollections']})
    secret = {'apiVersion': 'v1', 'kind': 'Secret',
              'metadata': {'name': 'eg-diagnostic-users-' + RUN_NAME, 'namespace': 'cma-scale'},
              'type': 'Opaque', 'stringData': credentials}
    kube('create', '-f', '-', data=secret)
    javascript = '''
try {
  const admin = db.getSiblingDB('admin');
  if (!admin.auth('bootstrap', process.env.ADMIN_PASSWORD)) throw new Error('auth');
  const target = admin.getSiblingDB(process.env.database);
  if (target.getCollectionNames().length !== 0) throw new Error('nonempty');
  for (const command of ROLES) {
    const result = target.runCommand(command);
    if (!result.ok) throw new Error('role');
  }
  for (const user of ['migrator','gateway','oracle']) {
    target.createUser({user, pwd:process.env[user], roles:[{
      role:user === 'migrator' ? 'cma_eg_migrator' : 'cma_eg_runtime',
      db:process.env.database}]});
  }
  const state = admin.runCommand({replSetGetStatus:1});
  const primary = state.members.find(m => m.state === 1);
  const match = primary && /^mongo-([0-2])\./.exec(primary.name);
  if (!match) throw new Error('primary');
  print(JSON.stringify({ready:true, users:3, primary_index:Number(match[1])}));
} catch (error) {
  print(JSON.stringify({ready:false, code:Number(error.code || 0)}));
  quit(1);
}
'''.replace('ROLES', json.dumps([runtime, migration]))
    hosts = ','.join(f'mongo-{i}.mongo.cma-scale.svc.cluster.local:27017' for i in range(3))
    job = {'apiVersion': 'batch/v1', 'kind': 'Job',
           'metadata': {'name': 'eg-diagnostic-provision-' + RUN_NAME, 'namespace': 'cma-scale'},
           'spec': {'backoffLimit': 0, 'activeDeadlineSeconds': 120,
                    'template': {'spec': {'restartPolicy': 'Never',
                      'automountServiceAccountToken': False,
                      'containers': [{'name': 'provision', 'image': 'mongo:8.0.13',
                        'command': ['mongosh', '--quiet',
                                    'mongodb://' + hosts + '/admin?replicaSet=rs0',
                                    '--eval', javascript],
                        'envFrom': [{'secretRef': {'name': 'eg-diagnostic-users-' + RUN_NAME}}],
                        'env': [{'name': 'ADMIN_PASSWORD', 'valueFrom': {
                            'secretKeyRef': {'name': 'mongo-bootstrap', 'key': 'ADMIN_PASSWORD'}}}],
                        'resources': {'requests': {'cpu': '25m', 'memory': '64Mi'},
                                      'limits': {'cpu': '200m', 'memory': '256Mi'}}}]}}}}
    kube('create', '-f', '-', data=job)
    try:
        kube('wait', '--for=condition=complete', 'job/eg-diagnostic-provision-' + RUN_NAME, '--timeout=125s')
        status = json.loads(kube('logs', 'job/eg-diagnostic-provision-' + RUN_NAME))
        if not status.get('ready'):
            raise RuntimeError('Mongo fixture user preparation failed')
        primary = status['primary_index']
        port = 28117 + primary
        try:
            with socket.create_connection(('127.0.0.1', port), timeout=.2):
                raise RuntimeError('Previous cohort Mongo forward is still active')
        except (ConnectionRefusedError, TimeoutError):
            pass
        logfile = open(PRIVATE / 'mongo-forward.log', 'ab', buffering=0)
        proc = subprocess.Popen(K + ['port-forward', '--address=127.0.0.1',
                                f'pod/mongo-{primary}', f'{port}:27017'],
                                stdout=logfile, stderr=logfile, start_new_session=True)
        logfile.close()
        for _ in range(50):
            if proc.poll() is not None:
                raise RuntimeError('Mongo forward exited during startup')
            try:
                with socket.create_connection(('127.0.0.1', port), timeout=.2):
                    break
            except (ConnectionRefusedError, TimeoutError):
                time.sleep(.1)
        else:
            proc.terminate()
            raise RuntimeError('Mongo forward did not become ready')
        private_json(PRIVATE / 'mongo-forward.json', {'pid': proc.pid,
                     'primary_index': primary, 'port': port})
        host_uri = lambda user: (f'mongodb://{user}:{credentials[user]}@127.0.0.1:{port}/'
                                f'{database}?authSource={database}&directConnection=true&maxPoolSize=4')
        gateway_uri = (f'mongodb://gateway:{credentials["gateway"]}@{hosts}/{database}'
                       f'?replicaSet=rs0&authSource={database}&maxPoolSize=10')
        private_json(PRIVATE / 'config.json', {
            'listen': '0.0.0.0:19100', 'advertised_origin': 'http://127.0.0.1:19100',
            'database': database, 'admin_mongodb_uri': host_uri('migrator'),
            'oracle_mongodb_uri': host_uri('oracle'), 'gateway_mongodb_uri': gateway_uri,
            'private_output': str(PRIVATE / 'output'),
            'control_bearer': secrets.token_hex(32), 'replicas_enabled': COHORT == 'on'})
        print('Prepared dedicated EG database grants and private fixture configuration; Mongo primary index', primary)
    finally:
        kube('delete', 'job', 'eg-diagnostic-provision-' + RUN_NAME, '--wait=false')


if __name__ == '__main__':
    main()
