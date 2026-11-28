import json
from pathlib import Path
import sys
import unittest

sys.path.insert(0, str(Path(__file__).resolve().parent / 'cma'))
import provider


class FilePreviewProviderTests(unittest.TestCase):
    def test_emits_one_fixed_relative_edit_with_an_exact_call_id(self):
        marker = 'WS4_FILE_' + 'a' * 32
        request = {'input': [{'type': 'additional_tools', 'tools': [
            {'name': 'functions', 'tools': [{'name': 'exec'}]},
        ]}]}
        events = provider.file_preview_tool_events('response', request, marker, 'fixture-call')
        self.assertEqual(events[-1]['type'], 'response.completed')
        self.assertEqual(len(events[-1]['response']['output']), 1)
        item = events[-1]['response']['output'][0]
        self.assertEqual(item['call_id'], 'fixture-call')
        self.assertEqual((item['type'], item['namespace'], item['name']), ('custom_tool_call', 'functions', 'exec'))
        patch = f'*** Begin Patch\n*** Add File: cma-preview-{"a" * 32}.txt\n+{marker}\n*** End Patch\n'
        self.assertEqual(item['input'], 'text(await tools.apply_patch(' + json.dumps(patch) + '));')
        with self.assertRaisesRegex(ValueError, 'fixture_file_preview_tool_undeclared'):
            provider.file_preview_tool_events('response', {'tools': [{'name': 'unrelated'}]}, marker, 'fixture-call')
        with self.assertRaisesRegex(ValueError, 'fixture_file_preview_invalid'):
            provider.file_preview_tool_events('response', request, marker + '/substitute', 'fixture-call')
