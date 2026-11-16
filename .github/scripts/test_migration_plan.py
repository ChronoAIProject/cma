#!/usr/bin/env python3
import copy
import json
import tempfile
from pathlib import Path
import unittest

import migration_plan


OLD = 'a' * 40
NEW = 'b' * 40
OLD_IMAGE = 'us-central1-docker.pkg.dev/example-project-01/example-registry-02/cma-api@sha256:' + '1' * 64
NEW_IMAGE = 'us-central1-docker.pkg.dev/example-project-01/example-registry-02/cma-api@sha256:' + '2' * 64


class MigrationPlanTests(unittest.TestCase):
    def inventory(self, names=('agents', 'sandbox_usage')):
        return {'collections': [{'name': name} for name in names]}

    def test_expands_every_collection_in_both_directions_and_policies(self):
        plan = migration_plan.build(self.inventory(), OLD, OLD_IMAGE, NEW, NEW_IMAGE)
        self.assertEqual(len(plan['fixtures']), 6)
        self.assertEqual(len(plan['cells']), 24)
        self.assertEqual({row['policy'] for row in plan['fixtures']}, {'retained', 'disabled', 'enabled'})
        self.assertEqual({row['role'] for row in plan['readers']}, {'historical', 'current', 'candidate', 'recovery'})
        self.assertEqual(sum(row['expectation'] == 'refuse' for row in plan['cells']), 0)
        self.assertEqual(plan['candidate']['image'], NEW_IMAGE)

    def test_slug_collisions_and_invalid_identity_fail_closed(self):
        with self.assertRaisesRegex(ValueError, 'collide'):
            migration_plan.build(self.inventory(('a_b', 'a-b')), OLD, OLD_IMAGE, NEW, NEW_IMAGE)
        with self.assertRaises(ValueError):
            migration_plan.build(self.inventory(), OLD, 'cma-api:latest', NEW, NEW_IMAGE)
        with self.assertRaises(ValueError):
            migration_plan.build({'collections': []}, OLD, OLD_IMAGE, NEW, NEW_IMAGE)

    def test_cli_create_only_and_hash(self):
        with tempfile.TemporaryDirectory() as directory:
            inventory = Path(directory) / 'inventory.json'
            output = Path(directory) / 'plan.json'
            inventory.write_text(json.dumps(self.inventory(('agents',))))
            migration_plan.main(['--inventory', str(inventory), '--old-source', OLD,
                                 '--old-image', OLD_IMAGE, '--new-source', NEW,
                                 '--new-image', NEW_IMAGE, '--output', str(output)])
            first = output.read_bytes()
            with self.assertRaises(FileExistsError):
                migration_plan.main(['--inventory', str(inventory), '--old-source', OLD,
                                     '--old-image', OLD_IMAGE, '--new-source', NEW,
                                     '--new-image', NEW_IMAGE, '--output', str(output)])
            self.assertEqual(output.read_bytes(), first)


if __name__ == '__main__':
    unittest.main()
