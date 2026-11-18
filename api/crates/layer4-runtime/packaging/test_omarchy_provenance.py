"""Arch attestation requires exact package, ownership, inventory and file bytes."""
import io
import tarfile
import tempfile
import unittest
from pathlib import Path
from types import SimpleNamespace
from unittest.mock import patch

import omarchy_provenance as provenance


class OmarchyProvenanceTests(unittest.TestCase):
    def setUp(self):
        self.package, self.data = b'synthetic package', b'synthetic library'
        self.expected = dict(package=dict(name='gnutls', version='3.8.13-3', architecture='x86_64',
                                          archive_sha256=provenance.sha(self.package)),
                             metadata=dict(root='var/lib/pacman/local/gnutls-3.8.13-3',
                                           mtree_sha256=provenance.sha(b'synthetic mtree'),
                                           archive_path='/cache/gnutls.pkg.tar.zst', package_hashes='package-hashes'),
                             files=[dict(path='usr/lib/example.so', bytes=len(self.data), sha256=provenance.sha(self.data))])

    def populate(self, root):
        base = root / self.expected['metadata']['root']
        base.mkdir(parents=True)
        (base / 'desc').write_text('%NAME%\ngnutls\n\n%VERSION%\n3.8.13-3\n\n%ARCH%\nx86_64\n')
        (base / 'files').write_text('%FILES%\nusr/lib/example.so\n')
        (base / 'mtree').write_bytes(b'synthetic mtree')
        (root / 'package-hashes').write_text(self.expected['package']['archive_sha256'] + '  /cache/gnutls.pkg.tar.zst\n')
        library = root / 'usr/lib/example.so'
        library.parent.mkdir(parents=True)
        library.write_bytes(self.data)
        return base, library

    def test_installed_package_and_all_file_evidence_required(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            base, library = self.populate(root)
            provenance.validate_installation(root, self.expected)
            for path in (base / 'desc', base / 'files', base / 'mtree', root / 'package-hashes', library):
                with self.subTest(field=path.name):
                    original = path.read_bytes()
                    path.write_bytes(b'changed')
                    with self.assertRaises(ValueError):
                        provenance.validate_installation(root, self.expected)
                    path.write_bytes(original)
            library.unlink()
            library.symlink_to(base / 'mtree')
            with self.assertRaises(ValueError):
                provenance.validate_installation(root, self.expected)

    def test_official_archive_member_hashes_and_unique_members_required(self):
        for duplicate in (False, True):
            data = io.BytesIO()
            with tarfile.open(fileobj=data, mode='w') as archive:
                for name, value in [('.MTREE', b'synthetic mtree'), ('usr/lib/example.so', self.data)] * (2 if duplicate else 1):
                    entry = tarfile.TarInfo(name)
                    entry.size = len(value)
                    archive.addfile(entry, io.BytesIO(value))
            with patch.object(provenance.subprocess, 'run', return_value=SimpleNamespace(stdout=data.getvalue())):
                if duplicate:
                    with self.assertRaises(ValueError):
                        provenance.validate_archive(self.package, self.expected)
                else:
                    provenance.validate_archive(self.package, self.expected)
                    with self.assertRaises(ValueError):
                        provenance.validate_archive(self.package + b'changed', self.expected)

    def test_base_package_without_cached_download_gets_verified_record(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            self.populate(root)
            history = root / 'package-hashes'
            history.write_text('unrelated package\n')
            with self.assertRaises(ValueError):
                provenance.validate_installation(root, self.expected)
            with patch.object(provenance, 'validate_archive') as validate:
                provenance.record_verified_archive(root, self.expected, self.package)
                validate.assert_called_once_with(self.package, self.expected)
            provenance.validate_installation(root, self.expected)
            self.assertTrue(history.read_text().startswith('unrelated package\n'))
            self.assertFalse((root / 'cache/gnutls.pkg.tar.zst').exists())

    def test_new_record_cannot_hide_content_or_existing_record_drift(self):
        for change in ('archive', 'file', 'record', 'duplicate', 'redirect'):
            with self.subTest(change=change), tempfile.TemporaryDirectory() as directory:
                root = Path(directory)
                _, library = self.populate(root)
                history = root / 'package-hashes'
                data = self.package
                if change == 'archive':
                    data += b'changed'
                if change == 'file':
                    library.write_bytes(b'changed')
                if change == 'record':
                    history.write_text('unreviewed  /cache/gnutls.pkg.tar.zst\n')
                if change == 'duplicate':
                    history.write_text(history.read_text() * 2)
                if change == 'redirect':
                    (root / 'cache').symlink_to(root / 'usr', target_is_directory=True)
                before = history.read_bytes()
                with patch.object(provenance, 'validate_archive'):
                    with self.assertRaises(ValueError):
                        provenance.record_verified_archive(root, self.expected, data)
                self.assertEqual(history.read_bytes(), before)
                self.assertFalse((root / 'cache/gnutls.pkg.tar.zst').exists())


if __name__ == '__main__':
    unittest.main()
