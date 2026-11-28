#!/usr/bin/env python3
"""The config digest is bound to the pinned manifest when Docker omits it."""
import hashlib
import json
from pathlib import Path
import tempfile
import unittest

import migration_kind_runner as runner
from test_migration_kind_runner import OLD


CONFIG = 'sha256:' + '6' * 64
REGISTRY = 'ghcr.io/example-owner/cma-agent-manager@'


def manifest(**overrides):
    document = {'schemaVersion': 2, 'mediaType': 'application/vnd.oci.image.manifest.v1+json',
                'config': {'mediaType': 'application/vnd.oci.image.config.v1+json', 'digest': CONFIG, 'size': 7},
                'layers': [{'mediaType': 'application/vnd.oci.image.layer.v1.tar+gzip',
                            'digest': 'sha256:' + '8' * 64, 'size': 9}]}
    document.update(overrides)
    return json.dumps(document, separators=(',', ':'))


def pinned(text):
    return REGISTRY + 'sha256:' + hashlib.sha256(text.encode()).hexdigest()


def row(image, descriptor=True, identity=None):
    value = {'Id': identity or image.split('@', 1)[1], 'Architecture': 'amd64', 'Os': 'linux',
             'RepoDigests': [image],
             'Config': {'Labels': {'org.opencontainers.image.revision': OLD}}}
    if descriptor:
        value['Descriptor'] = {'mediaType': 'application/vnd.oci.image.manifest.v1+json',
                               'digest': image.split('@', 1)[1], 'size': 1440}
    return value


def docker(image_row, raw):
    def command(argv):
        if argv[:3] == ['docker', 'image', 'inspect']:
            return json.dumps([image_row])
        if argv[:5] == ['docker', 'buildx', 'imagetools', 'inspect', '--raw']:
            return raw
        raise AssertionError(argv)
    return command


class ManifestConfigTests(unittest.TestCase):
    def test_containerd_store_binds_config_through_the_pinned_manifest(self):
        text = manifest()
        image = pinned(text)
        value = runner.checked_identity(docker(row(image), text), image, OLD)
        self.assertEqual(value['config'], CONFIG)
        self.assertEqual(value['local_image_id'], image.split('@', 1)[1])

    def test_classic_store_requires_its_image_id_to_be_the_manifest_config(self):
        text = manifest()
        image = pinned(text)
        value = runner.checked_identity(docker(row(image, descriptor=False, identity=CONFIG), text), image, OLD)
        self.assertEqual(value['config'], CONFIG)
        with self.assertRaisesRegex(runner.RunnerError, 'local image id does not match'):
            runner.checked_identity(docker(row(image, descriptor=False), text), image, OLD)

    def test_manifest_bytes_must_hash_to_the_pin(self):
        text = manifest()
        image = pinned(text)
        with self.assertRaisesRegex(runner.RunnerError, 'does not match its digest'):
            runner.checked_identity(docker(row(image), text + ' '), image, OLD)

    def test_local_descriptor_must_name_the_pinned_manifest(self):
        text = manifest()
        image = pinned(text)
        other = row(image)
        other['Descriptor']['digest'] = 'sha256:' + '9' * 64
        with self.assertRaisesRegex(runner.RunnerError, 'lacks an explicit immutable config digest'):
            runner.checked_identity(docker(other, text), image, OLD)
        index = row(image)
        index['Descriptor']['mediaType'] = 'application/vnd.oci.image.index.v1+json'
        with self.assertRaisesRegex(runner.RunnerError, 'lacks an explicit immutable config digest'):
            runner.checked_identity(docker(index, text), image, OLD)

    def test_an_index_is_refused_even_when_its_digest_matches(self):
        text = manifest(mediaType='application/vnd.oci.image.index.v1+json', manifests=[])
        image = pinned(text)
        with self.assertRaisesRegex(runner.RunnerError, 'single-platform manifest'):
            runner.checked_identity(docker(row(image, descriptor=False), text), image, OLD)

    def test_unreadable_manifest_is_a_named_refusal(self):
        text = manifest()
        image = pinned(text)

        def failing(argv):
            if argv[1] == 'buildx':
                raise runner.subprocess.CalledProcessError(1, argv)
            return json.dumps([row(image)])
        with self.assertRaisesRegex(runner.RunnerError, 'manifest is not readable'):
            runner.checked_identity(failing, image, OLD)

    def test_recorder_keeps_only_manifest_identity_and_returns_exact_bytes(self):
        text = manifest()
        image = pinned(text)
        with tempfile.TemporaryDirectory() as directory:
            recorder = runner.ImagePreflight(Path(directory), docker(row(image), text))
            self.assertEqual(runner.checked_identity(recorder, image, OLD)['config'], CONFIG)
            saved = json.loads((Path(directory) / 'image-preflight.private.json').read_text())
            rows = saved['observations']
            self.assertEqual([row_['argv'][1] for row_ in rows], ['image', 'buildx'])
            self.assertEqual(rows[0]['metadata'][0]['Descriptor'],
                             {'mediaType': 'application/vnd.oci.image.manifest.v1+json',
                              'digest': image.split('@', 1)[1], 'annotations': {}})
            self.assertEqual(rows[1]['metadata'], {
                'manifest_sha256': image.split(':')[-1],
                'mediaType': 'application/vnd.oci.image.manifest.v1+json', 'config': CONFIG})
            self.assertNotIn('8' * 64, json.dumps(saved))

    def test_recorder_still_refuses_other_buildx_commands(self):
        with tempfile.TemporaryDirectory() as directory:
            recorder = runner.ImagePreflight(Path(directory), lambda argv: '')
            for argv in (['docker', 'buildx', 'build', '.'],
                         ['docker', 'buildx', 'imagetools', 'create', '--raw', 'x'],
                         ['docker', 'buildx', 'imagetools', 'inspect', 'x']):
                with self.subTest(argv=argv), self.assertRaisesRegex(ValueError, 'only permits'):
                    recorder(argv)


class KindNodeImageTests(unittest.TestCase):
    def campaign(self, directory, *node):
        plan = Path(directory) / 'plan.json'
        plan.write_text('{}')
        return runner.Campaign(Path(directory) / 'run', 'kind-not-invoked', 1, plan.read_bytes(), *node)

    def test_default_node_image_is_unchanged(self):
        with tempfile.TemporaryDirectory() as directory:
            self.assertEqual(self.campaign(directory).node_image, 'kindest/node:v1.34.0')

    def test_supplied_node_image_must_be_digest_pinned(self):
        pinned_node = 'docker.io/kindest/node@sha256:' + '7' * 64
        with tempfile.TemporaryDirectory() as directory:
            self.assertEqual(self.campaign(directory, pinned_node).node_image, pinned_node)
            for value in ('kindest/node:v1.34.0-rc', 'kindest/node:latest', 'example.com/node@sha256:' + '7' * 64):
                with self.subTest(value=value), self.assertRaisesRegex(runner.RunnerError, 'pinned by digest'):
                    self.campaign(directory, value)

    def test_cluster_config_uses_the_selected_node_image(self):
        pinned_node = 'kindest/node:v1.34.0@sha256:' + '7' * 64
        with tempfile.TemporaryDirectory() as directory:
            campaign = self.campaign(directory, pinned_node)
            calls = []

            class Stop(Exception):
                pass

            def command(argv, **kwargs):
                calls.append(argv)
                raise Stop()
            campaign.command = command
            with self.assertRaises(Stop):
                campaign.create([])
            config = json.loads((campaign.output / 'kind.private.json').read_text())
            self.assertEqual(config['nodes'], [{'role': 'control-plane', 'image': pinned_node}])
            self.assertEqual(calls[0][:2], ['kind-not-invoked', 'create'])


class ImageLoadTests(unittest.TestCase):
    def campaign(self, directory, status):
        plan = Path(directory) / 'plan.json'
        plan.write_text('{}')
        campaign = runner.Campaign(Path(directory) / 'run', 'kind-bin', 1, plan.read_bytes())
        calls = []

        def command(argv, **kwargs):
            calls.append(argv)
            if argv[:2] == ['docker', 'info']:
                return status
            if argv[:2] == ['docker', 'save']:
                Path(argv[argv.index('--output') + 1]).write_bytes(b'archive')
            return ''
        campaign.command = command
        return campaign, calls

    def test_classic_store_keeps_kind_load_docker_image(self):
        with tempfile.TemporaryDirectory() as directory:
            campaign, calls = self.campaign(directory, '[["Backing Filesystem","extfs"]]')
            campaign.load_image('alias-a')
            campaign.load_image('alias-b')
            self.assertEqual(calls, [
                ['docker', 'info', '--format', '{{json .DriverStatus}}'],
                ['kind-bin', 'load', 'docker-image', '--name', campaign.cluster, 'alias-a'],
                ['kind-bin', 'load', 'docker-image', '--name', campaign.cluster, 'alias-b']])

    def test_containerd_store_loads_one_platform_archive_and_removes_it(self):
        with tempfile.TemporaryDirectory() as directory:
            campaign, calls = self.campaign(directory, '[["driver-type","io.containerd.snapshotter.v1"]]')
            campaign.load_image('alias-a')
            save, load = calls[1], calls[2]
            self.assertEqual(save[:4], ['docker', 'save', '--platform', 'linux/amd64'])
            archive = save[save.index('--output') + 1]
            self.assertEqual(load, ['kind-bin', 'load', 'image-archive', '--name', campaign.cluster, archive])
            self.assertTrue(archive.endswith('.private.tar'))
            self.assertFalse(Path(archive).exists())

    def test_failed_archive_load_still_removes_the_archive(self):
        with tempfile.TemporaryDirectory() as directory:
            campaign, calls = self.campaign(directory, '[["driver-type","io.containerd.snapshotter.v1"]]')
            saved = campaign.command

            def failing(argv, **kwargs):
                if argv[1:3] == ['load', 'image-archive']:
                    calls.append(argv)
                    raise runner.RunnerError('command failed: kind-bin')
                return saved(argv, **kwargs)
            campaign.command = failing
            with self.assertRaisesRegex(runner.RunnerError, 'command failed'):
                campaign.load_image('alias-a')
            self.assertFalse(any(Path(directory, 'run').glob('*.tar')))


if __name__ == '__main__':
    unittest.main()
