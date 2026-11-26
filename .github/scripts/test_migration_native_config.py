from copy import deepcopy
import hashlib
import json
from pathlib import Path
import tempfile
import unittest
from unittest.mock import patch

import migration_native_config as config
from migration_native_kind_fixture import fixture_config
from migration_native_process import NativeProcessError
import test_migration_native_images as image_fixture


class ConfigTests(unittest.TestCase):
    def setUp(self):
        self.temp=tempfile.TemporaryDirectory();self.addCleanup(self.temp.cleanup)
        self.root=Path(self.temp.name).resolve()
        self.path,self.value,self.pins,_=fixture_config(self.root)
        patcher=patch('migration_native_config.TLS_PINS',self.pins);patcher.start();self.addCleanup(patcher.stop)

    def test_input_binds_all_sources_archives_dependencies_and_private_tls(self):
        value=config.load(self.path)
        self.assertEqual(value['input_sha256'],hashlib.sha256(self.path.read_bytes()).hexdigest())
        self.assertEqual(value['artifacts'],self.value['artifacts'])
        self.assertEqual(len(value['images']),10)
        self.assertEqual(value['inventory_collections'],21)

    def test_invalid_scope_and_foreign_sources_fail_before_runtime(self):
        original=json.loads(self.path.read_text())
        for mutate in (lambda v:v.update(extra=True),lambda v:v.update(schema='other'),
                       lambda v:v.update(artifacts={}),lambda v:v.update(archives={}),
                       lambda v:v.update(docker_context='foreign context'),
                       lambda v:v['node'].update(architecture='other'),
                       lambda v:v['identity'].update(image=v['mongo']['image']),
                       lambda v:v['artifacts'].update(new=v['artifacts']['old']),
                       lambda v:v.update(tls={})):
            value=deepcopy(original);mutate(value);self.path.write_text(json.dumps(value))
            with self.assertRaises((ValueError,RuntimeError)):config.load(self.path)
        self.path.write_text(json.dumps(original))
        inventory=Path(original['inventory']);value=json.loads(inventory.read_text())
        value['new']['source']='b'*40;inventory.write_text(json.dumps(value))
        with self.assertRaisesRegex(NativeProcessError,'inventory'):config.load(self.path)

    def test_private_regular_paths_and_pins(self):
        self.path.chmod(0o644)
        with self.assertRaises(NativeProcessError):config.load(self.path)
        self.path.chmod(0o600)
        key=Path(self.value['tls']['private_key']);key.write_text('changed')
        with self.assertRaisesRegex(NativeProcessError,'pin'):config.load(self.path)
        with self.assertRaises(NativeProcessError):config.regular('relative')
        link=self.root/'link';link.symlink_to(self.path)
        with self.assertRaises(NativeProcessError):config.regular(str(link))
        with self.assertRaises(NativeProcessError):config.regular(str(self.path),1)

    def test_validated_archive_metadata_reads_exact_bytes(self):
        fixture=image_fixture.ImageTests();fixture.setUp()
        try:
            fixture.write()
            value=config.metadata_from_archive(fixture.path,fixture.expected['image'],fixture.expected['config'])
            self.assertEqual(value,[fixture.manifest,fixture.config])
        finally:fixture.doCleanups()


if __name__=='__main__':unittest.main()
