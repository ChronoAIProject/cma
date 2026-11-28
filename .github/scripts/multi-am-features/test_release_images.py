import copy
import hashlib
import io
import json
from pathlib import Path
import tarfile
import tempfile
import unittest

from release_images import checked_packet, file_hash, load_release_image, SOURCE


class ReleaseImageTests(unittest.TestCase):
    def setUp(self):
        temporary = tempfile.TemporaryDirectory()
        self.addCleanup(temporary.cleanup)
        self.root = Path(temporary.name)
        self.revision = 'a' * 40
        self.packet = {'schema': 'multi-am-release-images-v1', 'apiSource': self.revision,
                       'images': {}}
        for component in ('api', 'runtime', 'runtime-omarchy'):
            revision = self.revision if component == 'api' else 'b' * 40
            self.packet['images'][component] = self.archive(component, revision)
        self.path = self.root / 'images.json'

    def archive(self, component, revision, bad_member=False):
        labels = {'org.opencontainers.image.source': SOURCE,
                  'org.opencontainers.image.revision': revision}
        if component.startswith('runtime'):
            labels['dev.chrono.cma.runtime-contract'] = '3'
        config = json.dumps({'os': 'linux', 'architecture': 'amd64', 'config': {'Labels': labels}}).encode()
        config_hash = 'sha256:' + hashlib.sha256(config).hexdigest()
        layer = b'fixture-layer'
        layer_hash = 'sha256:' + hashlib.sha256(layer).hexdigest()
        manifest = json.dumps({'schemaVersion': 2, 'config': {'digest': config_hash, 'size': len(config)},
                              'layers': [{'digest': layer_hash, 'size': len(layer)}]}).encode()
        manifest_hash = 'sha256:' + hashlib.sha256(manifest).hexdigest()
        index = json.dumps({'schemaVersion': 2, 'manifests': [{'digest': manifest_hash,
                                                            'size': len(manifest)}]}).encode()
        path = self.root / (component + '.tar')
        with tarfile.open(path, 'w') as archive:
            files = {'index.json': index, 'blobs/sha256/' + config_hash[7:]: config,
                     'blobs/sha256/' + manifest_hash[7:]: manifest, 'blobs/sha256/' + layer_hash[7:]: layer}
            if bad_member:
                files['../outside'] = b'refused'
            for name, data in files.items():
                member = tarfile.TarInfo(name); member.size = len(data)
                archive.addfile(member, io.BytesIO(data))
        return {'platform': 'linux/amd64', 'sourceRevision': revision,
                'manifest': manifest_hash, 'config': config_hash, 'archive': path.name,
                'archiveBytes': path.stat().st_size, 'archiveSha256': file_hash(path)}

    def check(self):
        self.path.write_text(json.dumps(self.packet))
        return checked_packet(self.path, file_hash(self.path), self.revision)

    def test_exact_packet_allows_independently_pinned_runtime_sources(self):
        self.assertEqual(self.check(), self.packet)
        self.assertNotEqual(self.packet['images']['runtime']['sourceRevision'], self.revision)
        with self.assertRaisesRegex(ValueError, 'release_packet_hash'):
            checked_packet(self.path, '0' * 64, self.revision)

    def test_source_platform_and_digest_cannot_be_relabelled(self):
        original = copy.deepcopy(self.packet)
        for field, value, code in [('sourceRevision', 'c' * 40, 'release_api_source_mismatch'),
                                   ('platform', 'linux/arm64', 'release_image_platform_source'),
                                   ('config', 'sha256:' + '0' * 64, 'release_manifest_config_mismatch')]:
            self.packet = copy.deepcopy(original)
            self.packet['images']['api'][field] = value
            with self.assertRaisesRegex(ValueError, code):
                self.check()
        self.packet = original
        self.packet['images']['runtime']['sourceRevision'] = 'c' * 40
        with self.assertRaisesRegex(ValueError, 'release_image_source_labels'):
            self.check()

    def test_archive_corruption_escape_and_wrong_member_refuse_before_import(self):
        original = copy.deepcopy(self.packet)
        self.packet['images']['api']['archive'] = '../api.tar'
        with self.assertRaisesRegex(ValueError, 'release_archive_path'):
            self.check()
        self.packet = original
        with (self.root / 'api.tar').open('ab') as stream:
            stream.write(b'changed')
        with self.assertRaisesRegex(ValueError, 'release_archive_size'):
            self.check()
        self.packet['images']['api'] = self.archive('api', self.revision, bad_member=True)
        with self.assertRaisesRegex(ValueError, 'release_archive_unsafe_member'):
            self.check()

    def test_import_requires_manifest_and_config_and_revalidates_archive(self):
        self.check(); row = self.packet['images']['api']; calls = []

        def command(argv, **_):
            calls.append(argv)
            if argv[-2:] == ['images', 'list']:
                return 'local-image type ' + row['manifest'] + ' size linux/amd64 -'
            if 'inspecti' in argv:
                return json.dumps({'status': {'id': row['config']}})
            return ''

        result = load_release_image(command, 'owned-node', 'owned-cluster', self.root, 'api', row)
        self.assertEqual(result, 'localhost/multi-am-api@' + row['manifest'])
        self.assertEqual(calls[0][:3], ['kind', 'load', 'image-archive'])
        (self.root / row['archive']).write_bytes(b'changed')
        calls.clear()
        with self.assertRaisesRegex(ValueError, 'release_archive_size'):
            load_release_image(command, 'owned-node', 'owned-cluster', self.root, 'api', row)
        self.assertEqual(calls, [])
