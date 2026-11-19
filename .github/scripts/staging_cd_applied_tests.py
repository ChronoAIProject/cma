"""Only a matching projection from the pinned reader reaches deployment selection."""
import hashlib
import json
from pathlib import Path
import tempfile
import unittest
from unittest.mock import Mock

from staging_cd_applied import configured_group
from staging_cd_clock import timestamp
from staging_cd_common import Refusal
from staging_cd_compatibility import GROUP
from staging_cd_receipts_tests import fixture_receipt


class AppliedReaderTests(unittest.TestCase):
    def exercise(self, transform=None, *, missing=False):
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            scripts = root / "staging/scripts"
            scripts.mkdir(parents=True)
            reader = scripts / "applied_contract.py"
            if not missing:
                reader.touch()
            receipts = {k: fixture_receipt(component=k) for k in GROUP}
            def read(args, env, timeout, **options):
                self.assertEqual(args[2], str(reader))
                self.assertEqual(options["cwd"], root)
                self.assertNotIn("GH_TOKEN", env)
                self.assertLessEqual(timeout, 30)
                path = Path(args[-1])
                self.assertEqual(path.stat().st_mode & 0o777, 0o600)
                self.assertEqual(set(json.loads(path.read_bytes())), GROUP)
                value = {"schema": "cma-configured-contract-v1", "request": hashlib.sha256(path.read_bytes()).hexdigest(),
                         "configured": ["runtime"]}
                if transform:
                    transform(value)
                target = root / "applied-contract.json"
                target.write_text(json.dumps(value))
                target.chmod(0o600)
                return 0
            runner = Mock(side_effect=read)
            result = configured_group(scripts / "deploy_set.py", root, receipts, timestamp(), {}, runner=runner)
            runner.assert_called_once()
            return result

    def test_matches_are_configuration_only_and_request_bound(self):
        self.assertEqual(self.exercise(), {"runtime"})
        for override in ({"schema": "old"}, {"request": "0" * 64}, {"configured": ["web"]},
                         {"configured": ["am", "am"]}, {"applied": ["am"]}):
            with self.subTest(override=override), self.assertRaises(Refusal):
                self.exercise(lambda value: value.update(override))
        with self.assertRaisesRegex(Refusal, "unavailable"):
            self.exercise(missing=True)
