#!/usr/bin/env python3
from contextlib import redirect_stdout
import hashlib
import io
import json
import os
import subprocess
import sys
import tempfile
from pathlib import Path
import unittest

import migration_plan


OLD = 'a' * 40
NEW = 'b' * 40
OLD_IMAGE = 'us-central1-docker.pkg.dev/example-project-01/example-registry-02/cma-api@sha256:' + '1' * 64
NEW_IMAGE = 'us-central1-docker.pkg.dev/example-project-01/example-registry-02/cma-api@sha256:' + '2' * 64
ROLES = ('historical', 'current', 'candidate', 'recovery')
SCRIPT = Path(migration_plan.__file__)


def release_inputs(names=('promoted_records',)):
    artifacts = {role: {'source': source * 40,
                        'image': OLD_IMAGE.split('@')[0] + '@sha256:' + digit * 64}
                 for role, source, digit in zip(ROLES, 'acbd', '1324')}
    inventory = {'schema': 'cma-reader-writer-source-inventory-v1',
                 'old': {'source': OLD, 'tree': 'e' * 40},
                 'new': {'source': NEW, 'tree': 'f' * 40},
                 'collections': [{'name': name, 'old': {}, 'new': {}} for name in names]}
    cells = []
    for role in ROLES:
        for name in names:
            for policy in ('retained', 'disabled', 'enabled'):
                fixture = f"collection-{name.replace('_', '-')}-{policy}"
                cells.append({'id': f'{role}-{fixture}', 'reader': role, 'fixture': fixture,
                              'expectation': 'refuse' if role == 'historical'
                              and policy == 'enabled' else 'accept'})
    return inventory, artifacts, cells


class MigrationPlanTests(unittest.TestCase):
    def inventory(self, names=('agents', 'sandbox_usage')):
        return {'collections': [{'name': name} for name in names]}

    def test_release_preserves_four_distinct_artifacts_api(self):
        inventory, artifacts, expectations = release_inputs()
        plan = migration_plan.build_release(inventory, artifacts, expectations)
        self.assertEqual({row['role']: row['artifact'] for row in plan['readers']}, artifacts)
        self.assertNotEqual(artifacts['recovery'], artifacts['historical'])
        self.assertEqual(plan['candidate'], artifacts['candidate'])
        self.assertEqual(plan['cells'], expectations)
        self.assertEqual(len(plan['fixtures']), 3)
        for fixture in plan['fixtures']:
            role = 'historical' if fixture['policy'] == 'retained' else 'candidate'
            self.assertEqual(fixture['writer'], artifacts[role])

    def test_release_preserves_four_distinct_artifacts_cli(self):
        with tempfile.TemporaryDirectory(dir=SCRIPT.parent) as directory:
            root = Path(directory)
            args = []
            for flag, value in zip(('inventory', 'artifacts', 'expectations'), release_inputs()):
                path = root / f'{flag}.json'
                path.write_text(json.dumps(value))
                args.extend([f'--{flag}', str(path)])
            output = root / 'plan.json'
            result = subprocess.run([sys.executable, '-B', str(SCRIPT), *args,
                                     '--output', str(output)], capture_output=True, text=True)
            self.assertEqual(result.returncode, 0, result.stderr)
            plan = json.loads(output.read_bytes())
            self.assertEqual({row['role']: row['artifact'] for row in plan['readers']},
                             release_inputs()[1])

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
        with tempfile.TemporaryDirectory(dir=SCRIPT.parent) as directory:
            inventory = Path(directory) / 'inventory.json'
            output = Path(directory) / 'plan.json'
            inventory.write_text(json.dumps(self.inventory(('agents',))))
            summary = io.StringIO()
            with redirect_stdout(summary):
                migration_plan.main(['--inventory', str(inventory), '--old-source', OLD,
                                     '--old-image', OLD_IMAGE, '--new-source', NEW,
                                     '--new-image', NEW_IMAGE, '--output', str(output)])
            first = output.read_bytes()
            self.assertEqual(json.loads(summary.getvalue()), {
                'mode': 'pairwise-probe', 'execution': 'NOT_RUN', 'fixtures': 3,
                'cells': 12, 'sha256': hashlib.sha256(first).hexdigest()})
            with self.assertRaises(FileExistsError):
                migration_plan.main(['--inventory', str(inventory), '--old-source', OLD,
                                     '--old-image', OLD_IMAGE, '--new-source', NEW,
                                     '--new-image', NEW_IMAGE, '--output', str(output)])
            self.assertEqual(output.read_bytes(), first)


class ReleaseValidationTests(unittest.TestCase):
    def setUp(self):
        directory = tempfile.TemporaryDirectory(dir=SCRIPT.parent)
        self.addCleanup(directory.cleanup)
        self.root = Path(directory.name)
        self.output = self.root / 'plan.json'
        self.reset_inputs()

    def reset_inputs(self):
        self.inventory, self.artifacts, self.expectations = release_inputs()

    def build(self):
        return migration_plan.build_release(self.inventory, self.artifacts, self.expectations)

    def write_inputs(self):
        args = []
        for flag in ('inventory', 'artifacts', 'expectations'):
            path = self.root / f'{flag}.json'
            path.write_text(json.dumps(getattr(self, flag)))
            args.extend([f'--{flag}', str(path)])
        return [*args, '--output', str(self.output)]

    def run_cli(self, args):
        return subprocess.run([sys.executable, '-B', str(SCRIPT), *args],
                              capture_output=True, text=True, timeout=10)

    def assert_safe_failure(self, args):
        result = self.run_cli(args)
        self.assertIn(result.returncode, (1, 2))
        self.assertEqual(result.stdout, '')
        self.assertNotIn(str(self.root), result.stderr)
        self.assertNotIn('private-input-marker', result.stderr)
        self.assertNotIn('Traceback', result.stderr)
        self.assertFalse(self.output.exists())

    def assert_refused(self):
        with self.assertRaises(ValueError):
            self.build()
        self.assert_safe_failure(self.write_inputs())

    def test_missing_unknown_and_malformed_roles_fail_without_output(self):
        for role in ROLES:
            with self.subTest(missing=role):
                self.reset_inputs()
                del self.artifacts[role]
                self.assert_refused()
        for value in (None, [], {}, 'private-input-marker'):
            with self.subTest(role_value=value):
                self.reset_inputs()
                self.artifacts['current'] = value
                self.assert_refused()
        for location in ('extra-role', 'extra-field'):
            with self.subTest(location=location):
                self.reset_inputs()
                target = self.artifacts if location == 'extra-role' else self.artifacts['recovery']
                target['private-input-marker'] = True
                self.assert_refused()

    def test_invalid_identities_and_components_fail_without_output(self):
        for field, values in (
            ('source', ('develop', 'a' * 39, 'A' * 40, None, [])),
            ('image', ('cma-api:latest', 'sha256:' + '1' * 64, OLD_IMAGE + '\n',
                       OLD_IMAGE.replace('/cma-api@', '/cma-web@'), None, {})),
        ):
            for value in values:
                with self.subTest(field=field, value=value):
                    self.reset_inputs()
                    self.artifacts['current'][field] = value
                    self.assert_refused()

    def test_inventory_schema_and_pinned_sources_are_required(self):
        mutations = [
            lambda i: i.pop('schema'), lambda i: i.update(schema='private-input-marker'),
            lambda i: i.pop('old'), lambda i: i.pop('new'),
            lambda i: i['old'].update(source=NEW), lambda i: i['new'].update(source=OLD),
            lambda i: i['old'].update(source='develop'),
            lambda i: i['new'].update(tree='short'), lambda i: i['old'].update(extra=True),
        ]
        for mutate in mutations:
            with self.subTest(mutation=mutate):
                self.reset_inputs()
                mutate(self.inventory)
                self.assert_refused()

    def test_same_old_source_cannot_bind_a_different_image(self):
        for role in ('current', 'candidate', 'recovery'):
            with self.subTest(role=role):
                self.reset_inputs()
                self.artifacts[role]['source'] = OLD
                if role == 'candidate':
                    self.inventory['new']['source'] = OLD
                self.assert_refused()

    def test_equal_artifacts_must_be_explicit_and_add_no_samples(self):
        self.artifacts['current'] = dict(self.artifacts['historical'])
        self.artifacts['recovery'] = dict(self.artifacts['candidate'])
        plan = self.build()
        self.assertEqual(len(plan['cells']), 12)
        self.assertEqual({r['role']: r['artifact'] for r in plan['readers']}, self.artifacts)
        self.assertEqual(set(plan), {'schema', 'candidate', 'readers', 'fixtures', 'cells'})
        self.assertTrue(all(set(c) == {'id', 'reader', 'fixture', 'expectation'} for c in plan['cells']))
        for role in ROLES:
            self.artifacts[role] = dict(self.artifacts['historical'])
        self.inventory['new']['source'] = OLD
        self.assertEqual(len(self.build()['readers']), 4)
        del self.artifacts['recovery']
        self.assert_refused()

    def test_expectations_are_complete_unique_and_predeclared(self):
        mutations = [
            lambda e: e.pop(), lambda e: e.append(dict(e[0])),
            lambda e: e[1].update(id=e[0]['id']),
            lambda e: e[1].update(reader=e[0]['reader'], fixture=e[0]['fixture']),
            lambda e: e[0].update(reader='foreign'), lambda e: e[0].update(fixture='foreign'),
            lambda e: e[0].update(expectation='private-input-marker'),
            lambda e: e[0].update(expectation={}), lambda e: e[0].update(extra=True),
            lambda e: e[0].pop('expectation'), lambda e: e[0].update(id='a' * 97),
            lambda e: e[0].update(id=''), lambda e: e[0].update(reader=[]),
            lambda e: e[6].update(expectation='refuse'), lambda e: e[9].update(expectation='refuse'),
        ]
        for mutate in mutations:
            with self.subTest(mutation=mutate):
                self.reset_inputs()
                mutate(self.expectations)
                self.assert_refused()
        for value in (None, [], {}, [None], self.expectations * 400):
            with self.subTest(invalid_list=type(value)):
                self.reset_inputs()
                self.expectations = value
                self.assert_refused()

    def test_reviewed_ids_and_refusals_survive_reordering(self):
        self.expectations[3]['expectation'] = 'refuse'
        self.expectations[0]['id'] = 'reviewed-retained-case'
        expected = self.build()
        self.expectations.reverse()
        self.assertEqual(self.build(), expected)
        self.assertEqual(sum(c['expectation'] == 'refuse' for c in expected['cells']), 2)

    def test_fixture_ids_hashes_and_writers_stay_source_bound(self):
        plan = self.build()
        probe = migration_plan.build(self.inventory, OLD, OLD_IMAGE, NEW, NEW_IMAGE)
        self.assertEqual(plan['fixtures'], probe['fixtures'])
        self.assertEqual([f['sha256'] for f in plan['fixtures']], [
            '3de656a4e8003a05d65f9a9c4222d4ee99f6ea7bf232532bb07e4f2ca77c810f',
            'a9e39f0b82c3fb06884aaab56a8586786f2123b086b615ebbc5f8d87c1412415',
            'c2412a8898c4f23ed6f0a57e6ba2968038d3fa29561561e0f6ef3aafd1289ac4'])
        self.assertEqual(probe['readers'][1]['artifact'], probe['candidate'])
        self.assertEqual(probe['readers'][3]['artifact'], probe['readers'][0]['artifact'])
        self.artifacts['current'] = dict(self.artifacts['recovery'])
        self.assertEqual(self.build()['fixtures'], plan['fixtures'])
        self.artifacts['historical']['image'] = OLD_IMAGE.replace('1' * 64, '5' * 64)
        changed = self.build()['fixtures']
        self.assertEqual([f['id'] for f in changed], [f['id'] for f in plan['fixtures']])
        self.assertNotEqual(changed[0]['sha256'], plan['fixtures'][0]['sha256'])
        self.assertEqual(changed[1:], plan['fixtures'][1:])

    def test_collection_and_id_bounds_match_the_matrix(self):
        self.inventory, self.artifacts, self.expectations = release_inputs(
            tuple(f'collection_{n}' for n in range(341)))
        plan = self.build()
        self.assertEqual(len(plan['fixtures']), 1023)
        self.assertEqual(len(plan['cells']), 4092)
        self.inventory, self.artifacts, self.expectations = release_inputs(
            tuple(f'collection_{n}' for n in range(342)))
        with self.assertRaisesRegex(ValueError, 'cell limit'):
            self.build()
        for rows in (None, {}, [], [None], [{'name': []}], [{'name': ''}],
                     [{'name': 'a'}] * 2, [{'name': 'a_b'}, {'name': 'a-b'}],
                     [{'name': 'a' * 81}], [{'name': 'a' * 77}]):
            with self.subTest(rows=rows):
                self.reset_inputs()
                self.inventory['collections'] = rows
                self.assert_refused()
        with self.assertRaises(ValueError):
            migration_plan.build({'collections': [{'name': 'a' * 66}]}, OLD, OLD_IMAGE, NEW, NEW_IMAGE)

    def test_cli_modes_are_exclusive_and_never_fall_back(self):
        args = self.write_inputs()
        probe = ['--old-source', OLD, '--old-image', OLD_IMAGE,
                 '--new-source', NEW, '--new-image', NEW_IMAGE]
        for invalid in (args[:2] + args[4:], args[:4] + args[6:], args + probe,
                        args + probe[:2], args[:2] + args[6:],
                        args[:2] + args[6:] + probe[:-2], args + ['--private-input-marker'],
                        args + ['--art', 'private-input-marker'], args + ['--old-image']):
            with self.subTest(args=invalid):
                self.assert_safe_failure(invalid)
        help_result = self.run_cli(['--help'])
        self.assertEqual(help_result.returncode, 0)
        self.assertIn('pairwise probe', help_result.stdout)
        self.assertIn('release declaration', help_result.stdout)

    def test_cli_rejects_ambiguous_or_malformed_json(self):
        for flag in ('inventory', 'artifacts', 'expectations'):
            for data in ('{"private-input-marker":', '{"duplicate":1,"duplicate":2}',
                         '{"nested":{"source":1,"source":2}}', 'NaN', 'null', '[' * 1100):
                with self.subTest(flag=flag, data=data[:30]):
                    args = self.write_inputs()
                    (self.root / f'{flag}.json').write_text(data)
                    self.assert_safe_failure(args)

    def test_duplicate_valid_keys_cannot_silently_select_the_last_value(self):
        for flag, key in (('inventory', 'schema'), ('artifacts', 'historical'),
                          ('artifacts', 'source'), ('expectations', 'expectation')):
            with self.subTest(flag=flag, key=key):
                args = self.write_inputs()
                path = self.root / f'{flag}.json'
                data = path.read_text().replace(f'"{key}":',
                                               f'"{key}": "private-input-marker", "{key}":', 1)
                path.write_text(data)
                self.assert_safe_failure(args)

    def test_cli_inputs_are_bounded_regular_files(self):
        for kind in ('empty', 'oversized', 'invalid-utf8', 'missing', 'directory', 'symlink', 'fifo'):
            with self.subTest(kind=kind):
                args = self.write_inputs()
                path = self.root / 'artifacts.json'
                path.unlink()
                if kind == 'empty':
                    path.touch()
                elif kind == 'oversized':
                    with path.open('wb') as stream:
                        stream.truncate(4 * 1024 * 1024 + 1)
                elif kind == 'invalid-utf8':
                    path.write_bytes(b'\xff')
                elif kind == 'directory':
                    path.mkdir()
                elif kind == 'symlink':
                    path.symlink_to(self.root / 'inventory.json')
                elif kind == 'fifo':
                    os.mkfifo(path)
                self.assert_safe_failure(args)
                if kind == 'directory':
                    path.rmdir()
                elif kind != 'missing':
                    path.unlink()

    def test_cli_create_only_byte_hash_and_safe_summary(self):
        args = self.write_inputs()
        result = self.run_cli(args)
        self.assertEqual(result.returncode, 0, result.stderr)
        first = self.output.read_bytes()
        self.assertEqual(json.loads(result.stdout), {
            'mode': 'release-declaration', 'execution': 'NOT_RUN', 'fixtures': 3,
            'cells': 12, 'sha256': hashlib.sha256(first).hexdigest()})
        self.assertEqual(result.stderr, '')
        result = self.run_cli(args)
        self.assertEqual(result.returncode, 1)
        self.assertEqual(result.stdout, '')
        self.assertNotIn(str(self.output), result.stderr)
        self.assertEqual(self.output.read_bytes(), first)
        self.artifacts = {}
        result = self.run_cli(self.write_inputs())
        self.assertEqual(result.returncode, 1)
        self.assertEqual(self.output.read_bytes(), first)

    def test_cli_refuses_oversized_output_before_creating_it(self):
        self.inventory, self.artifacts, self.expectations = release_inputs(
            tuple(f'collection_{n}' for n in range(170)))
        for artifact in self.artifacts.values():
            artifact['image'] = artifact['image'].replace('/cma-api@', '/' + 'a' * 10000 + '@')
        self.assert_safe_failure(self.write_inputs())


if __name__ == '__main__':
    unittest.main()
