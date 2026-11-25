"""Dependency selection must remain finite and reject mixed fixture authority."""
import argparse
import io
from types import SimpleNamespace
import unittest
from unittest.mock import Mock, patch

from migration_fixture_dependencies import DEPENDENCIES, add_arguments, configure, capture


class DependencyTests(unittest.TestCase):
    def test_every_declared_overlay_selects_its_exact_renderer(self):
        for name, (module, _) in DEPENDENCIES.items():
            with self.subTest(name=name), patch('migration_fixture_dependencies.import_module') as load:
                original, rendered = object(), object()
                load.return_value.manifests = Mock(return_value=rendered)
                parser = argparse.ArgumentParser()
                add_arguments(parser)
                args = parser.parse_args(['--' + name.replace('_', '-')])
                self.assertIs(configure(original, args), rendered)
                load.assert_called_once_with(module)
                load.return_value.manifests.assert_called_once_with(original)

    def test_default_does_not_load_or_change_dependencies(self):
        with patch('migration_fixture_dependencies.import_module') as load:
            objects = []
            self.assertIs(configure(objects, SimpleNamespace()), objects)
            load.assert_not_called()

    def test_mixed_authority_is_rejected_by_cli_and_direct_call(self):
        parser = argparse.ArgumentParser()
        add_arguments(parser)
        with patch('sys.stderr', io.StringIO()), self.assertRaises(SystemExit):
            parser.parse_args(['--directory-fixture', '--bot-fixture'])
        with self.assertRaisesRegex(ValueError, 'mutually exclusive'):
            configure([], SimpleNamespace(directory_fixture=True, bot_fixture=True))

    def test_renderer_failure_is_preserved(self):
        with patch('migration_fixture_dependencies.import_module') as load:
            load.return_value.manifests.side_effect = ValueError('invalid authority')
            with self.assertRaisesRegex(ValueError, 'invalid authority'):
                configure([], SimpleNamespace(directory_fixture=True))

    def test_capture_only_runs_for_explicit_fixture(self):
        with patch('migration_launch_documents.capture') as read:
            self.assertIsNone(capture('campaign', SimpleNamespace(), 'namespace', 'mongo', 'stage', 'source', 'api'))
            read.assert_not_called()
            read.return_value = {'outcome': 'PASS'}
            result = capture('campaign', SimpleNamespace(visitor_fixture=True), 'namespace', 'mongo',
                             'stage', 'source', 'api')
            self.assertEqual(result, {'outcome': 'PASS'})
            read.assert_called_once_with('campaign', 'namespace', 'mongo', 'stage', 'source', 'api')
        with patch('migration_sandbox_documents.capture', return_value={'outcome': 'PASS'}) as read:
            result = capture('campaign', SimpleNamespace(sandbox_control_fixture=True), 'namespace', 'mongo',
                             'stage', 'source', 'api')
            self.assertEqual(result, {'outcome': 'PASS'})
            read.assert_called_once_with('campaign', 'namespace', 'mongo', 'stage', 'source', 'api')


if __name__ == '__main__':
    unittest.main()
