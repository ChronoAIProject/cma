import io
from pathlib import Path
import tempfile
import unittest
from unittest.mock import patch

import tls_fixture


class TLSFixtureTests(unittest.TestCase):
    def test_public_archive_request_is_bounded_and_identifies_ci(self):
        with patch('tls_fixture.urllib.request.urlopen', return_value=io.BytesIO(b'public archive')) as opened:
            self.assertEqual(tls_fixture.download(tls_fixture.ARCHIVE_URL), b'public archive')
        request = opened.call_args.args[0]
        self.assertEqual(request.full_url, 'https://get.localhost.direct/localhost.direct.OP.zip')
        self.assertIn('cma-two-am-ci', request.get_header('User-agent'))
        self.assertEqual(opened.call_args.kwargs, {'timeout': 30})

    def test_oversized_download_is_rejected(self):
        with patch('tls_fixture.urllib.request.urlopen', return_value=io.BytesIO(b'x' * (4 * 1024 * 1024 + 1))):
            with self.assertRaisesRegex(ValueError, 'public_tls_bundle_size'):
                tls_fixture.download(tls_fixture.ARCHIVE_URL)

    def test_rotated_or_substituted_bundle_never_reaches_extraction_or_tls_commands(self):
        with tempfile.TemporaryDirectory() as directory, \
                patch('tls_fixture.download', return_value=b'changed archive') as download, \
                patch('tls_fixture.zipfile.ZipFile') as extract, \
                patch('tls_fixture.subprocess.run') as command:
            destination = Path(directory) / 'tls'
            with self.assertRaisesRegex(ValueError, 'public_tls_bundle_rotated_requires_review'):
                tls_fixture.prepare(destination)
            self.assertEqual(list(destination.iterdir()), [])
            self.assertEqual(download.call_count, 1)
            extract.assert_not_called()
            command.assert_not_called()


if __name__ == '__main__':
    unittest.main()
