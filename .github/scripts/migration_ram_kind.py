#!/usr/bin/env python3
"""Run one disposable matrix kind node with /var on a private tmpfs volume.

The retained Mongo instance survives all application reader switches. Node or
host failure durability is outside this fixture's claim. MATRIX_REAL_KIND names
the ordinary kind binary; no shared cluster or global Docker setting changes.
"""
import json
import os
from pathlib import Path
import re
import secrets
import subprocess
import sys
import time

from migration_kind_manifests import IDENTITY

CLUSTER = re.compile(r'cma-matrix-[a-f0-9]{10}')
OWNER = 'cma.migration.storage-owner'
SCOPE = 'cma.migration.cluster'


def option(args, name):
    if args.count(name) != 1 or args.index(name) + 1 == len(args):
        raise ValueError('missing or repeated option: ' + name)
    return args[args.index(name) + 1]


def scoped_names(cluster):
    if not CLUSTER.fullmatch(cluster):
        raise ValueError('RAM kind requires an owned matrix cluster name')
    return {key: cluster + suffix for key, suffix in (
        ('volume', '-ram-var'), ('holder', '-ram-holder'),
        ('seed', '-ram-seed'), ('check', '-ram-check'))}


def ram_config(config, mountpoint):
    if (not isinstance(mountpoint, str) or not mountpoint.startswith('/')
            or ':' in mountpoint or '\n' in mountpoint):
        raise ValueError('invalid Docker volume mountpoint')
    config = json.loads(json.dumps(config))
    nodes = config.get('nodes', [])
    if len(nodes) != 1 or nodes[0].get('role') != 'control-plane':
        raise ValueError('RAM matrix requires one control-plane node')
    mounts = nodes[0].setdefault('extraMounts', [])
    if any(m.get('containerPath') == '/var' for m in mounts):
        raise ValueError('existing /var mount must not be replaced')
    mounts.append({'hostPath': mountpoint, 'containerPath': '/var'})
    return config


def require_owner(row, state, container=False):
    labels = row.get('Config', {}).get('Labels', {}) if container else row.get('Labels', {})
    if labels.get(OWNER) != state['nonce'] or labels.get(SCOPE) != state['cluster']:
        raise RuntimeError('resource ownership does not match; refusing mutation')


class RamKind:
    def __init__(self, real_kind, args):
        self.real_kind, self.args = real_kind, list(args)
        self.cluster = option(args, '--name')
        self.names = scoped_names(self.cluster)
        self.kubeconfig = Path(option(args, '--kubeconfig')).resolve()
        self.state_path = self.kubeconfig.parent / 'ram-kind-storage.json'
        self.state = None
        self.deadline = time.monotonic() + (220 if args[0] == 'create' else 160)

    def save(self):
        with os.fdopen(os.open(self.state_path, os.O_WRONLY | os.O_CREAT | os.O_TRUNC, 0o600), 'w') as output:
            os.fchmod(output.fileno(), 0o600)
            json.dump(self.state, output, indent=2)
            output.write('\n')

    def command(self, args, timeout=30):
        started = time.time()
        timeout = min(timeout, self.deadline - time.monotonic())
        if timeout <= 0:
            raise RuntimeError('RAM kind operation deadline exceeded')
        result = subprocess.run(args, text=True, capture_output=True, timeout=timeout)
        if self.state is not None:
            self.state['commands'].append({'argv': args, 'exit': result.returncode,
                                           'elapsed_seconds': time.time() - started})
            self.save()
        if result.returncode:
            raise RuntimeError('command failed: ' + args[0] + ': ' + result.stderr.strip())
        return result.stdout

    def volume_rows(self):
        names = self.command(['docker', 'volume', 'ls', '--format', '{{.Name}}']).splitlines()
        if self.names['volume'] not in names:
            return []
        return json.loads(self.command(['docker', 'volume', 'inspect', self.names['volume']]))

    def container_rows(self, name):
        ids = self.command(['docker', 'ps', '-a', '--filter', 'name=^/' + name + '$',
                            '--format', '{{.ID}}']).splitlines()
        if not ids:
            return []
        if len(ids) != 1:
            raise RuntimeError('container identity is not unique')
        return json.loads(self.command(['docker', 'inspect', ids[0]]))

    def labels(self):
        return ['--label', OWNER + '=' + self.state['nonce'],
                '--label', SCOPE + '=' + self.cluster]

    def create(self):
        config_path = Path(option(self.args, '--config')).resolve()
        if config_path.parent != self.kubeconfig.parent or self.state_path.exists():
            raise ValueError('fresh config and kubeconfig must share the output directory')
        config = json.loads(config_path.read_text())
        ram_config(config, '/validate-only')
        if self.cluster in self.command([self.real_kind, 'get', 'clusters']).splitlines():
            raise RuntimeError('cluster already exists')
        if self.volume_rows() or any(self.container_rows(n) for k, n in self.names.items() if k != 'volume'):
            raise RuntimeError('storage resource already exists')
        self.state = {'schema': 'migration-ram-kind-v1', 'cluster': self.cluster,
                      'nonce': secrets.token_hex(16), 'names': self.names,
                      'filesystem_limit_bytes': 6 * 1024**3,
                      'node_memory_limit_bytes': 8 * 1024**3, 'node_cpus': 2,
                      'kind_invoked': False, 'cleanup_completed': False, 'commands': []}
        self.save()
        self.command(['docker', 'volume', 'create', *self.labels(), '--driver', 'local',
                      '--opt', 'type=tmpfs', '--opt', 'device=tmpfs',
                      '--opt', 'o=size=6g,noswap', self.names['volume']])
        row = self.volume_rows()[0]
        require_owner(row, self.state)
        if (row.get('Driver') != 'local' or row.get('Options') !=
                {'type': 'tmpfs', 'device': 'tmpfs', 'o': 'size=6g,noswap'}):
            raise RuntimeError('storage driver does not match the RAM budget')
        mountpoint = row['Mountpoint']
        # Keep the local-driver tmpfs mounted while kind uses its bind path.
        self.command(['docker', 'run', '--detach', '--rm', '--name', self.names['holder'],
                      *self.labels(), '--platform', 'linux/amd64', '--network', 'none',
                      '--read-only', '--memory', '64m', '--memory-swap', '64m', '--cpus', '0.1',
                      '--pull', 'never', '--volume', self.names['volume'] + ':/hold',
                      '--entrypoint', '/bin/sleep', IDENTITY, '4200'])
        # Docker copies the node image's preloaded /var data into the empty
        # named volume. The later bind mount must not hide those initial files.
        node_image = config['nodes'][0]['image']
        self.command(['docker', 'run', '--rm', '--name', self.names['seed'], *self.labels(),
                      '--network', 'none', '--read-only', '--memory', '64m', '--cpus', '0.1',
                      '--pull', 'never', '--volume', self.names['volume'] + ':/var',
                      '--entrypoint', '/bin/true', node_image], timeout=90)
        filesystem = self.command(['docker', 'run', '--rm', '--name', self.names['check'],
                      *self.labels(), '--platform', 'linux/amd64', '--network', 'none',
                      '--read-only', '--memory', '64m', '--cpus', '0.1', '--pull', 'never',
                      '--volume', self.names['volume'] + ':/probe', '--entrypoint', '/usr/bin/stat',
                      IDENTITY, '-f', '-c', '%T', '/probe']).strip()
        if filesystem != 'tmpfs':
            raise RuntimeError('storage is not actually tmpfs')
        self.state['filesystem_verified'] = filesystem
        self.state['mountpoint'] = mountpoint
        prepared = config_path.with_name('kind.ram.private.json')
        prepared.write_text(json.dumps(ram_config(config, mountpoint), indent=2) + '\n')
        args = list(self.args)
        args[args.index('--config') + 1] = str(prepared)
        self.state['kind_invoked'] = True
        self.save()
        print(self.command([self.real_kind, *args], timeout=180), end='')
        node = self.cluster + '-control-plane'
        rows = self.container_rows(node)
        if len(rows) != 1 or rows[0]['Config']['Labels'].get('io.x-k8s.kind.cluster') != self.cluster:
            raise RuntimeError('kind node identity mismatch')
        self.command(['docker', 'update', '--cpus', '2', '--memory', '8g',
                      '--memory-swap', '8g', node])
        row = self.container_rows(node)[0]
        host = row['HostConfig']
        if host['Memory'] != 8 * 1024**3 or host['MemorySwap'] != 8 * 1024**3 or host['NanoCpus'] != 2_000_000_000:
            raise RuntimeError('kind node budget readback mismatch')
        self.state['node_budget_verified'] = True
        self.save()

    def delete(self):
        # A failed freshness check creates no receipt and confers no authority
        # to delete an existing cluster or a similarly named volume.
        self.state = json.loads(self.state_path.read_text())
        if self.state.get('cluster') != self.cluster or self.state.get('names') != self.names:
            raise RuntimeError('storage receipt scope mismatch')
        volumes = self.volume_rows()
        for row in volumes:
            require_owner(row, self.state)
        if self.state['kind_invoked']:
            print(self.command([self.real_kind, *self.args], timeout=120), end='')
        if self.container_rows(self.cluster + '-control-plane'):
            raise RuntimeError('node remains; refusing to tear down its storage')
        for key in ('check', 'seed', 'holder'):
            for row in self.container_rows(self.names[key]):
                require_owner(row, self.state, container=True)
                self.command(['docker', 'rm', '-f', row['Id']])
        if volumes:
            # Re-read ownership immediately before the destructive operation.
            for row in self.volume_rows():
                require_owner(row, self.state)
            self.command(['docker', 'volume', 'rm', self.names['volume']])
        if self.volume_rows() or any(self.container_rows(n) for k, n in self.names.items() if k != 'volume'):
            raise RuntimeError('owned storage cleanup incomplete')
        self.state['cleanup_completed'] = True
        self.save()


def main(args=None):
    os.umask(0o077)
    args = sys.argv[1:] if args is None else args
    real_kind = os.environ['MATRIX_REAL_KIND']
    if Path(real_kind).resolve() == Path(__file__).resolve():
        raise ValueError('real kind must not resolve to this adapter')
    if args[:2] in (['create', 'cluster'], ['delete', 'cluster']):
        tool = RamKind(real_kind, args)
        tool.create() if args[0] == 'create' else tool.delete()
    elif args[:2] == ['load', 'docker-image']:
        scoped_names(option(args, '--name'))
        os.execv(real_kind, [real_kind, *args])
    elif args == ['get', 'clusters']:
        os.execv(real_kind, [real_kind, *args])
    else:
        raise ValueError('unsupported RAM kind operation')


if __name__ == '__main__':
    main()
