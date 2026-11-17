#!/usr/bin/env python3
"""Storage isolation contracts; real Docker/kind execution is recorded separately."""
import json
import os
from pathlib import Path
import tempfile
import time
import unittest
from unittest.mock import patch

import migration_ram_kind as ram


class DockerFixture:
    def __init__(self, tool):
        self.tool = tool
        self.volume = None
        self.containers = {}
        self.calls = []
        self.filesystem = 'tmpfs'
        self.kind_fails = False
        self.preserve_node = False

    def labels(self, args):
        return dict(args[i + 1].split('=', 1) for i, v in enumerate(args) if v == '--label')

    def command(self, args, timeout=None):
        self.calls.append(args)
        if args[0] == '/kind':
            if args[1:3] == ['get', 'clusters']:
                return ''
            node = self.tool.cluster + '-control-plane'
            if args[1] == 'create':
                self.containers[node] = {'Id': node, 'Config': {'Labels': {
                    'io.x-k8s.kind.cluster': self.tool.cluster}}, 'HostConfig': {}}
                if self.kind_fails:
                    raise RuntimeError('kind creation failed')
            elif not self.preserve_node:
                self.containers.pop(node, None)
            return ''
        if args[1:3] == ['volume', 'ls']:
            return '' if self.volume is None else self.volume['Name'] + '\n'
        if args[1:3] == ['volume', 'create']:
            self.volume = {'Name': args[-1], 'Labels': self.labels(args), 'Driver': 'local',
                           'Options': {'type': 'tmpfs', 'device': 'tmpfs', 'o': 'size=6g,noswap'},
                           'Mountpoint': '/var/lib/docker/volumes/' + args[-1] + '/_data'}
            return args[-1]
        if args[1:3] == ['volume', 'inspect']:
            return json.dumps([self.volume])
        if args[1:3] == ['volume', 'rm']:
            self.volume = None
            return ''
        if args[1] == 'ps':
            name = args[args.index('--filter') + 1].removeprefix('name=^/').removesuffix('$')
            return name if name in self.containers else ''
        if args[1] == 'inspect':
            return json.dumps([self.containers[args[-1]]])
        if args[1] == 'run':
            name = args[args.index('--name') + 1]
            if '--detach' in args:
                self.containers[name] = {'Id': name, 'Config': {'Labels': self.labels(args)}}
            return self.filesystem if '/usr/bin/stat' in args else ''
        if args[1] == 'update':
            self.containers[args[-1]]['HostConfig'] = {
                'Memory': 8 * 1024**3, 'MemorySwap': 8 * 1024**3, 'NanoCpus': 2_000_000_000}
            return ''
        if args[1] == 'rm':
            del self.containers[args[-1]]
            return ''
        raise AssertionError('unexpected fixture command: ' + repr(args))


class RamKindTests(unittest.TestCase):
    def setUp(self):
        self.directory = tempfile.TemporaryDirectory()
        self.addCleanup(self.directory.cleanup)
        self.root = Path(self.directory.name)
        self.config = self.root / 'kind.json'
        self.config.write_text(json.dumps({'kind': 'Cluster', 'nodes': [
            {'role': 'control-plane', 'image': 'kindest/node:v1.34.0'}]}))
        self.tool = ram.RamKind('/kind', ['create', 'cluster', '--name', 'cma-matrix-0123456789',
            '--config', str(self.config), '--kubeconfig', str(self.root / 'kubeconfig')])
        self.docker = DockerFixture(self.tool)
        self.patcher = patch.object(self.tool, 'command', side_effect=self.docker.command)
        self.patcher.start()
        self.addCleanup(self.patcher.stop)

    def delete(self):
        self.tool.args = ['delete', 'cluster', '--name', self.tool.cluster,
                          '--kubeconfig', str(self.tool.kubeconfig)]
        self.tool.delete()

    def test_ram_configuration_preserves_other_mounts_and_refuses_replacement(self):
        config = json.loads(self.config.read_text())
        config['nodes'][0]['extraMounts'] = [{'hostPath': '/one', 'containerPath': '/two'}]
        result = ram.ram_config(config, '/owned')
        self.assertEqual(len(config['nodes'][0]['extraMounts']), 1)
        self.assertEqual(result['nodes'][0]['extraMounts'][-1],
                         {'hostPath': '/owned', 'containerPath': '/var'})
        with self.assertRaisesRegex(ValueError, 'existing /var'):
            ram.ram_config(result, '/another')
        for path in ('relative', '/bad:option', '/bad\nline', None):
            with self.subTest(path=path), self.assertRaises(ValueError):
                ram.ram_config(config, path)

    def test_scope_and_required_arguments_reject_foreign_targets(self):
        for name in ('cma-ws4-1723', 'cma-matrix-short', '../elsewhere', 'cma-matrix-01234567890'):
            with self.subTest(name=name), self.assertRaises(ValueError):
                ram.scoped_names(name)
        for args in ([], ['--name'], ['--name', 'one', '--name', 'two']):
            with self.assertRaises(ValueError):
                ram.option(args, '--name')

    def test_create_binds_actual_tmpfs_and_budget_then_cleanup_is_scoped(self):
        original = self.config.read_bytes()
        self.tool.create()
        saved = json.loads(self.tool.state_path.read_text())
        self.assertEqual(saved['filesystem_verified'], 'tmpfs')
        self.assertTrue(saved['node_budget_verified'])
        self.assertEqual(self.config.read_bytes(), original)
        self.assertEqual(self.tool.state_path.stat().st_mode & 0o777, 0o600)
        self.assertIn('--memory-swap', next(c for c in self.docker.calls if c[1] == 'update'))
        self.delete()
        self.assertIsNone(self.docker.volume)
        self.assertEqual(self.docker.containers, {})
        self.assertTrue(json.loads(self.tool.state_path.read_text())['cleanup_completed'])

    def test_failed_kind_boot_keeps_owned_storage_until_scoped_cleanup(self):
        self.docker.kind_fails = True
        with self.assertRaisesRegex(RuntimeError, 'kind creation failed'):
            self.tool.create()
        self.assertIsNotNone(self.docker.volume)
        self.delete()
        self.assertIsNone(self.docker.volume)
        self.assertEqual(self.docker.containers, {})

    def test_wrong_filesystem_never_starts_kind_and_can_clean_up(self):
        self.docker.filesystem = 'ext4'
        with self.assertRaisesRegex(RuntimeError, 'not actually tmpfs'):
            self.tool.create()
        self.assertFalse(any(c[:3] == ['/kind', 'create', 'cluster'] for c in self.docker.calls))
        self.delete()
        self.assertEqual(self.docker.containers, {})
        self.assertIsNone(self.docker.volume)

    def test_foreign_volume_owner_refuses_every_delete(self):
        self.tool.create()
        self.docker.volume['Labels'][ram.OWNER] = 'different-owner'
        before = len(self.docker.calls)
        with self.assertRaisesRegex(RuntimeError, 'ownership does not match'):
            self.delete()
        self.assertFalse(any(c[:3] == ['/kind', 'delete', 'cluster'] or c[1] == 'rm'
                             for c in self.docker.calls[before:]))

    def test_foreign_holder_is_never_removed(self):
        self.tool.create()
        holder = self.tool.names['holder']
        self.docker.containers[holder]['Config']['Labels'][ram.OWNER] = 'different-owner'
        with self.assertRaisesRegex(RuntimeError, 'ownership does not match'):
            self.delete()
        self.assertIn(holder, self.docker.containers)
        self.assertIsNotNone(self.docker.volume)

    def test_node_absence_required_before_storage_removal(self):
        self.tool.create()
        self.docker.preserve_node = True
        with self.assertRaisesRegex(RuntimeError, 'node remains'):
            self.delete()
        self.assertIn(self.tool.names['holder'], self.docker.containers)
        self.assertIsNotNone(self.docker.volume)

    def test_missing_receipt_does_not_authorize_cleanup(self):
        with self.assertRaises(FileNotFoundError):
            self.delete()
        self.assertEqual(self.docker.calls, [])

    def test_expired_operation_deadline_does_not_invoke_a_command(self):
        self.tool.deadline = time.monotonic() - 1
        with patch.object(ram.subprocess, 'run') as run:
            with self.assertRaisesRegex(RuntimeError, 'deadline exceeded'):
                ram.RamKind.command(self.tool, ['docker', 'version'])
            run.assert_not_called()

    def test_cli_refuses_unscoped_mutations_and_recursive_binary(self):
        with patch.dict(os.environ, {'MATRIX_REAL_KIND': '/kind'}):
            for args in (['delete', 'nodes'], ['load', 'docker-image', '--name', 'foreign']):
                with self.assertRaises(ValueError):
                    ram.main(args)
        with patch.dict(os.environ, {'MATRIX_REAL_KIND': str(Path(ram.__file__).resolve())}):
            with self.assertRaisesRegex(ValueError, 'must not resolve'):
                ram.main(['get', 'clusters'])


if __name__ == '__main__':
    unittest.main()
