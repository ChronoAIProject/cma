"""Package and installed-byte attestation with synthetic public input only."""
import io
import tarfile
import tempfile
import unittest
from pathlib import Path
from types import SimpleNamespace
from unittest.mock import patch

import package_provenance as provenance


class PackageProvenanceTests(unittest.TestCase):
    def setUp(self):
        self.content = b'synthetic public library'
        self.package = b'synthetic distribution package'
        self.expected = dict(package=dict(name='example', version='1', architecture='amd64',
                                          sha256=provenance.sha(self.package)),
                             library=dict(path='usr/lib/example.so', sha256=provenance.sha(self.content)))

    def archive(self, *, name=None, duplicate=False, link=False, content=None):
        stream = io.BytesIO()
        with tarfile.open(fileobj=stream, mode='w:xz') as archive:
            for _ in range(2 if duplicate else 1):
                entry = tarfile.TarInfo(name or './' + self.expected['library']['path'])
                data = self.content if content is None else content
                entry.size = len(data)
                if link:
                    entry.type, entry.linkname = tarfile.SYMTYPE, 'different.so'
                archive.addfile(entry, io.BytesIO(data))
        return stream.getvalue()

    def test_package_requires_whole_digest_and_unique_regular_library(self):
        with patch.object(provenance.subprocess, 'run', return_value=SimpleNamespace(stdout=self.archive())):
            provenance.validate_package(self.package, self.expected)
            with self.assertRaises(ValueError):
                provenance.validate_package(self.package + b'changed', self.expected)
        for options in (dict(name='elsewhere'), dict(duplicate=True), dict(link=True), dict(content=b'changed')):
            with self.subTest(options=options), \
                 patch.object(provenance.subprocess, 'run', return_value=SimpleNamespace(stdout=self.archive(**options))), \
                 self.assertRaises(ValueError):
                provenance.validate_package(self.package, self.expected)

    def test_installed_identity_bytes_and_regular_file_are_required(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            library = root / self.expected['library']['path']
            library.parent.mkdir(parents=True)
            library.write_bytes(self.content)
            installed = 'example\n1\namd64\n'
            provenance.validate_installation(root, self.expected, installed)
            for changed in ('different\n1\namd64\n', 'example\n2\namd64\n', 'example\n1\narm64\n'):
                with self.assertRaises(ValueError):
                    provenance.validate_installation(root, self.expected, changed)
            library.write_bytes(self.content + b'changed')
            with self.assertRaises(ValueError):
                provenance.validate_installation(root, self.expected, installed)
            library.unlink()
            target = root / 'copy.so'
            target.write_bytes(self.content)
            library.symlink_to(target)
            with self.assertRaises(ValueError):
                provenance.validate_installation(root, self.expected, installed)


if __name__ == '__main__':
    unittest.main()
