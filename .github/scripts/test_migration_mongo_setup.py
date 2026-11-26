"""Retain the first setup error instead of replaying partial role creation."""
import json
import os
from pathlib import Path
import secrets
import subprocess
import tempfile
import unittest

from migration_kind_manifests import manifests, mongo_documents
from test_migration_kind_runner import IMAGE, MONGO


class MongoSetupTests(unittest.TestCase):
    def test_partial_creation_failure_is_fatal_and_not_hidden_by_ping(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            shell = root / 'init.sh'
            shell.write_text(mongo_documents({'createRole': 'cma_am_runtime', 'roles': [],
                                               'privileges': []}))
            executable = root / 'mongosh'
            executable.write_text('''#!/usr/bin/env python3
import json,os,sys
from pathlib import Path
root=Path(os.environ['TEST_SETUP_ROOT'])
statement=sys.argv[-1]
with (root/'steps.jsonl').open('a') as stream:
    stream.write(json.dumps(statement)+'\\n')
if 'createRole' in statement:
    if (root/'role-created').exists():
        print('Role already exists',file=sys.stderr);sys.exit(1)
    (root/'role-created').touch();print('{"ok":1}')
elif 'createUser' in statement:
    print('original setup failure after role creation',file=sys.stderr);sys.exit(1)
elif 'ismaster' in statement:
    print('true')
else:
    print('1')
''')
            executable.chmod(0o700)
            env = {**os.environ, 'PATH': str(root) + os.pathsep + os.environ['PATH'],
                   'TEST_SETUP_ROOT': str(root), 'MATRIX_ADMIN_PASSWORD': secrets.token_urlsafe(32),
                   'MATRIX_RUNTIME_PASSWORD': secrets.token_urlsafe(32)}
            first = subprocess.run(['/bin/sh', str(shell)], env=env, capture_output=True, text=True, timeout=5)
            self.assertEqual(first.returncode, 1)
            self.assertIn('original setup failure', first.stderr)
            steps = [json.loads(row) for row in (root/'steps.jsonl').read_text().splitlines()]
            self.assertIn('createUser', steps[-1])
            self.assertEqual(sum('createRole' in row for row in steps), 1)
            # This is a deterministic demonstration of why Kubernetes must not
            # rerun the initializer after a partial setup failure.
            replay = subprocess.run(['/bin/sh', str(shell)], env=env, capture_output=True, text=True, timeout=5)
            self.assertEqual(replay.returncode, 1)
            self.assertIn('Role already exists', replay.stderr)
            self.assertNotIn('original setup failure', replay.stderr)
            for result in (first, replay):
                for key in ('MATRIX_ADMIN_PASSWORD', 'MATRIX_RUNTIME_PASSWORD'):
                    self.assertNotIn(env[key], result.stdout + result.stderr)

    def test_initializer_has_one_attempt_and_finite_lifetime(self):
        objects = manifests(IMAGE, secrets.token_urlsafe(32), secrets.token_urlsafe(32),
                            secrets.token_urlsafe(32), MONGO,
                            {'createRole': 'cma_am_runtime', 'roles': [], 'privileges': []})
        job = next(obj for obj in objects if obj['kind'] == 'Job' and obj['metadata']['name'] == 'mongo-init')
        self.assertEqual(job['spec']['backoffLimit'], 0, 'failed setup must not be recreated')
        self.assertEqual(job['spec']['template']['spec']['restartPolicy'], 'Never',
                         'the initial failure log must remain available')
        self.assertEqual(job['spec']['activeDeadlineSeconds'], 180)


if __name__ == '__main__':
    unittest.main()
