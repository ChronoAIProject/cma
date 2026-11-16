import hashlib
import unittest
from provider_probe import metadata


class RequestMetadata(unittest.TestCase):
    def test_instruction_marker_is_not_taken_from_user_content(self):
        marker = 'PROMPT-aaaaaaaa-bbbb-cccc-dddd-eeeeeeeeeeee'
        body = {'model': 'local-model', 'reasoning': {'effort': 'high'},
                'input': [{'role': 'user', 'content': [{'text': marker}]}]}
        value = metadata(body, 'case')
        self.assertEqual(value['instruction_marker_sha256'], [])
        body['input'].insert(0, {'role': 'developer', 'content': [{'text': 'Instruction ' + marker}]})
        value = metadata(body, 'case')
        self.assertEqual(value['instruction_marker_sha256'], [hashlib.sha256(marker.encode()).hexdigest()])
        self.assertNotIn(marker, str(value))

    def test_only_allowlisted_fields_and_hashes_leave_probe(self):
        body = {'model': 'test-model', 'instructions': 'Private instructions',
                'reasoning': {'effort': 'low'}, 'authorization': 'must-not-leak', 'secret': 'must-not-leak'}
        value = metadata(body, 'private-case')
        self.assertEqual(value['model'], 'test-model')
        self.assertEqual(value['reasoning_effort'], 'low')
        self.assertNotIn('Private instructions', str(value))
        self.assertNotIn('private-case', str(value))
        self.assertNotIn('must-not-leak', str(value))


if __name__ == '__main__':
    unittest.main()
