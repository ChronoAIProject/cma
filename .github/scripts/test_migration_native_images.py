from copy import deepcopy
import io
import json
from pathlib import Path
import tarfile
import tempfile
import unittest
from unittest.mock import patch

import migration_native_images as images


def encode(value):
    return json.dumps(value, separators=(',', ':')).encode()


def archive_fixture():
    source = 'a' * 40
    layer = b'fixture-layer-bytes'
    config = encode({'os': 'linux', 'architecture': 'amd64',
                     'config': {'Labels': {'org.opencontainers.image.revision': source}}})
    def descriptor(raw, media):
        return {'digest': images.sha256(raw), 'size': len(raw), 'mediaType': media}
    manifest = encode({'schemaVersion': 2, 'mediaType': images.MANIFEST_TYPES[0],
                       'config': descriptor(config, 'application/vnd.oci.image.config.v1+json'),
                       'layers': [descriptor(layer, 'application/vnd.oci.image.layer.v1.tar+gzip')]})
    image = 'registry.test/runtime@' + images.sha256(manifest)
    selected = descriptor(manifest, images.MANIFEST_TYPES[0])
    selected['annotations'] = {'io.containerd.image.name': image}
    index = {'schemaVersion': 2, 'manifests': [selected]}
    files = {'oci-layout': encode({'imageLayoutVersion': '1.0.0'}), 'index.json': encode(index)}
    for raw in (manifest, config, layer): files['blobs/sha256/' + images.sha256(raw)[7:]] = raw
    return files, {'image': image, 'source': source, 'config': images.sha256(config)}, manifest, config


class ImageTests(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory()
        self.addCleanup(self.temp.cleanup)
        self.path = Path(self.temp.name) / 'image.tar'
        self.files, self.expected, self.manifest, self.config = archive_fixture()

    def write(self, files=None, extra=None):
        with tarfile.open(self.path, mode='w') as archive:
            for name, raw in (files or self.files).items():
                entry = tarfile.TarInfo(name);entry.size = len(raw)
                archive.addfile(entry, io.BytesIO(raw))
            if extra is not None: archive.addfile(extra)

    def test_exact_raw_manifest_config_layers_and_named_registry_binding(self):
        self.write()
        result = images.validate_archive(self.path, self.expected)
        self.assertEqual(result['import_status'], 'NOT_RUN')
        self.assertEqual(result['manifest_sha256'], self.expected['image'].split('@')[1])
        self.assertEqual(result['config_sha256'], self.expected['config'])
        self.assertEqual(result['layers_verified'], 1)
        self.assertEqual(result['archive_sha256'], images.sha256(self.path.read_bytes())[7:])
        dependency = {k: v for k, v in self.expected.items() if k != 'source'}
        self.assertEqual(images.validate_archive(self.path, dependency)['artifact'], dependency)

    def test_rejects_changed_layer_manifest_and_config(self):
        for name in [n for n in self.files if n.startswith('blobs/')]:
            files = dict(self.files)
            if files[name] == self.manifest:
                files[name] = files[name].replace(b'"schemaVersion":2', b'"schemaVersion":3')
            elif files[name] == self.config:
                files[name] = files[name].replace(b'linux', b'other')
            else:
                files[name] = b'bad-layer-same-len!!'
            self.write(files)
            with self.assertRaises((ValueError, images.ImageImportError)):
                images.validate_archive(self.path, self.expected)

    def test_index_selection_and_platform_must_match(self):
        for mutate in (
                lambda i: i.update(schemaVersion=1), lambda i: i.update(manifests=[]),
                lambda i: i['manifests'].append(deepcopy(i['manifests'][0])),
                lambda i: i['manifests'][0].update(mediaType='other'),
                lambda i: i['manifests'][0].update(annotations={}),
                lambda i: i['manifests'][0].update(platform={'os': 'linux', 'architecture': 'arm64'}),
                lambda i: i['manifests'][0].update(size=1),
        ):
            files = dict(self.files);index = json.loads(files['index.json']);mutate(index)
            files['index.json'] = encode(index);self.write(files)
            with self.assertRaises(images.ImageImportError): images.validate_archive(self.path, self.expected)

    def test_archive_is_never_extracted_and_rejects_paths_links_or_unused_blobs(self):
        for name, kind in [('outside', tarfile.REGTYPE), ('../outside', tarfile.REGTYPE),
                           ('blobs', tarfile.SYMTYPE), ('oci-layout', tarfile.REGTYPE),
                           ('index.json', tarfile.DIRTYPE)]:
            entry = tarfile.TarInfo(name);entry.type = kind;entry.linkname = '/tmp/foreign'
            self.write(extra=entry)
            with self.assertRaises(images.ImageImportError): images.validate_archive(self.path, self.expected)
        files = dict(self.files);files['blobs/sha256/'+'0'*64] = b'unreferenced';self.write(files)
        with self.assertRaisesRegex(images.ImageImportError, 'unreferenced'):
            images.validate_archive(self.path, self.expected)
        self.assertEqual(sorted(p.name for p in Path(self.temp.name).iterdir()), ['image.tar'])

    def test_identity_symlink_metadata_and_size_bounds(self):
        self.write()
        for expected in (None, {}, {**self.expected, 'image': 'tag'}, {**self.expected, 'config': 'tag'}):
            with self.assertRaises(images.ImageImportError): images.validate_archive(self.path, expected)
        alias = self.path.with_name('symlink');alias.symlink_to(self.path)
        with self.assertRaises(images.ImageImportError): images.validate_archive(alias, self.expected)
        with patch.object(images, 'MAX_ARCHIVE', 1):
            with self.assertRaises(images.ImageImportError): images.validate_archive(self.path, self.expected)
        with patch.object(images, 'MAX_METADATA', 1):
            with self.assertRaises(images.ImageImportError): images.validate_archive(self.path, self.expected)
        for value in (None, {}, {'digest': 'bad', 'size': 1}, {'digest': 'sha256:'+'a'*64, 'size': True}):
            with self.assertRaises(images.ImageImportError): images.descriptor(value)

    def command(self, calls, mode=None):
        inspect_count = 0
        def command(argv):
            nonlocal inspect_count
            calls.append(argv)
            if argv[:3] == ['docker', 'context', 'inspect']:
                return json.dumps([{'Name': 'desktop-linux', 'Endpoints': {'docker': {
                    'Host': 'tcp://remote:2376' if mode == 'remote' else 'unix:///fixture/docker.sock'}}}])
            if 'inspect' in argv:
                inspect_count += 1
                return json.dumps([{'Id': ('b' if mode == 'replaced' and inspect_count > 1 else 'a') * 64,
                    'Name': '/cma-matrix-123456789a-control-plane', 'State': {'Running': True},
                    'Config': {'Labels': {'io.x-k8s.kind.cluster':
                        'foreign' if mode == 'foreign' else 'cma-matrix-123456789a'}}}])
            if argv[0] == 'env': return ''
            if 'content' in argv:
                if argv[-1] == self.expected['config']:
                    return 'mismatch' if mode == 'config' else self.config
                return 'mismatch' if mode == 'manifest' else self.manifest
            if 'inspecti' in argv:
                return json.dumps({'status': {'id': 'other' if mode == 'cri' else self.expected['config'],
                                             'repoDigests': [self.expected['image']]}})
            raise AssertionError('unexpected command')
        return command

    def test_import_requires_opt_in_then_rechecks_raw_bytes_and_cri(self):
        self.write();calls = []
        arguments = {'path': self.path, 'expected': self.expected, 'context': 'desktop-linux',
                     'cluster': 'cma-matrix-123456789a', 'owner_node_id': 'a'*64}
        with self.assertRaises(images.ImageImportError): images.import_archive(self.command(calls), **arguments)
        self.assertEqual(calls, [])
        result = images.import_archive(self.command(calls), **arguments, enabled=True)
        self.assertEqual(result['import_status'], 'EXACT_IMAGE_IMPORTED')
        self.assertTrue(result['manifest_preserved'])
        self.assertEqual(sum(c[0] == 'env' for c in calls), 1)
        self.assertNotIn('tag', str(calls))
        self.assertNotIn('kubectl', str(calls))
        for call in calls:
            if 'exec' in call: self.assertEqual(call[4], 'a'*64)
        for mode in ('remote', 'foreign', 'replaced', 'manifest', 'config', 'cri'):
            with self.subTest(mode=mode), self.assertRaises(images.ImageImportError):
                images.import_archive(self.command([], mode), **arguments, enabled=True)
        for owner in ('', 'b'*64):
            with self.assertRaises(images.ImageImportError):
                images.import_archive(self.command([]), **{**arguments, 'owner_node_id': owner}, enabled=True)

    def test_node_requires_owned_name_and_local_context(self):
        for context in ('', 'context with spaces', None):
            with self.assertRaises(images.ImageImportError):
                images.verify_node(self.command([]), context, 'cma-matrix-123456789a',
                                   'cma-matrix-123456789a-control-plane')
        with self.assertRaises(ValueError):
            images.verify_node(self.command([]), 'desktop-linux', 'foreign', 'foreign-control-plane')


if __name__ == '__main__':
    unittest.main()
