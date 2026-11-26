"""The Web result job fails unless each named Web job succeeded."""

import json
import os
import unittest
from unittest.mock import patch

import web_result


def needs(**overrides):
    value = {name: {"result": "success", "outputs": {}} for name in web_result.JOBS}
    for name, result in overrides.items():
        name = name.replace("_", "-")
        if result is None:
            value.pop(name)
        else:
            value[name] = {"result": result, "outputs": {}}
    return value


class WebResultTests(unittest.TestCase):
    def test_all_five_successes_pass(self):
        self.assertIsNone(web_result.check(needs()))

    def test_skipped_cancelled_or_failed_children_fail(self):
        for result in ("skipped", "cancelled", "failure", "", "SUCCESS"):
            with self.subTest(result=result):
                self.assertIn("web-conversation", web_result.check(needs(web_conversation=result)))

    def test_absent_extra_or_malformed_needs_fail(self):
        self.assertIsNotNone(web_result.check(needs(web_design=None)))
        self.assertIsNotNone(web_result.check(dict(needs(), extra={"result": "success"})))
        for value in (None, [], "success", {"web": "success"}):
            with self.subTest(value=value):
                self.assertIsNotNone(web_result.check(value))

    def test_entrypoint_exit_codes(self):
        with patch.dict(os.environ, {"NEEDS": json.dumps(needs())}):
            self.assertEqual(web_result.main(), 0)
        with patch.dict(os.environ, {"NEEDS": json.dumps(needs(web=None))}):
            self.assertEqual(web_result.main(), 1)
        with patch.dict(os.environ, {"NEEDS": "not json"}):
            self.assertEqual(web_result.main(), 1)


if __name__ == "__main__":
    unittest.main()
