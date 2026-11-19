"""All current publication paths use the same five images and Omarchy recipe."""
import copy
import json
from pathlib import Path
import runpy
import unittest

import yaml

from deployables import registry

ROOT = Path(__file__).resolve().parents[1]
COMPONENTS = ['api', 'web', 'runtime', 'cmaeg', 'runtime-omarchy']
TARGET = "${{ matrix.component == 'runtime-omarchy' && 'omarchy' || matrix.component == 'runtime' && 'default-runtime' || 'runtime' }}"
DESKTOP = "RUNTIME_DESKTOP=${{ matrix.component == 'runtime-omarchy' && '1' || '0' }}"


def validate(ci, publish):
    jobs = ci['jobs']
    grouped = {key: json.loads(jobs[key]['with']['components'])
               for key in ('am-images', 'eg-images', 'web-images', 'runtime-images')}
    if grouped != {'am-images': ['api'], 'eg-images': ['cmaeg'], 'web-images': ['web'], 'runtime-images': ['runtime']}:
        raise ValueError('current CI image matrix differs')
    if jobs['omarchy']['uses'] != './.github/workflows/ci-omarchy.yml':
        raise ValueError('Omarchy image qualification missing')
    for job in ('images', 'image-exports'):
        if publish['jobs'][job]['strategy']['matrix']['component'] != COMPONENTS:
            raise ValueError('current publication matrix differs')
    steps = publish['jobs']['images']['steps']
    builds = [step for step in steps if step.get('uses', '').startswith('docker/build-push-action@')]
    if len(builds) != 2:
        raise ValueError('native scan and exact publication archive required')
    for step in builds:
        settings = step['with']
        if settings['target'] != TARGET or DESKTOP not in settings['build-args'].splitlines() \
                or 'OMARCHY_CMA_BASE=runtime' not in settings['build-args'].splitlines():
            raise ValueError('Omarchy build would use the wrong variant')
        if settings['push'] is not False or 'image-manager' in settings['file']:
            raise ValueError('publication bypass or retired recipe')
    names = [step.get('name') for step in steps]
    if not names.index('Build the complete publication archive') < names.index('Scan exact archive and publish without rebuilding'):
        raise ValueError('exact archive scan must precede publication')


class PublicationScopeTests(unittest.TestCase):
    def setUp(self):
        self.ci = yaml.safe_load((ROOT / 'workflows/ci.yml').read_text())
        self.publish = yaml.safe_load((ROOT / 'workflows/publish.yml').read_text())

    def test_all_paths_select_exactly_five_and_build_real_omarchy(self):
        validate(self.ci, self.publish)
        self.assertEqual({recipe['component'] for recipe in registry().values()}, set(COMPONENTS))
        recipe = registry()['cma-runtime-omarchy']
        self.assertEqual(recipe['target'], 'omarchy')
        self.assertEqual(recipe['build_args'], {'RUNTIME_DESKTOP': '1', 'OMARCHY_CMA_BASE': 'runtime'})

    def test_omarchy_archive_producer_tracks_exactly_its_dependency_paths(self):
        workflow = yaml.safe_load((ROOT / 'workflows/ci-omarchy.yml').read_text())
        events = workflow.get('on', workflow.get(True))
        self.assertEqual(set(events), {'workflow_call', 'push'})
        policy = runpy.run_path(str(ROOT / 'scripts/omarchy-cache-key.py'))
        inputs = policy['manifest'](ROOT.parent)['inputs']
        self.assertEqual(set(events['push']), {'branches', 'paths'})
        self.assertEqual(events['push']['branches'], ['develop'])
        self.assertCountEqual(events['push']['paths'], [policy['DOCKERFILE'], *inputs])

    def test_retired_matrix_entries_and_missing_omarchy_are_rejected(self):
        for job in ('images', 'image-exports'):
            for retired in ('image-manager', 'runtime-desktop'):
                changed = copy.deepcopy(self.publish)
                changed['jobs'][job]['strategy']['matrix']['component'][-1] = retired
                with self.assertRaises(ValueError):
                    validate(self.ci, changed)
        for job in ('am-images', 'runtime-images'):
            changed = copy.deepcopy(self.ci)
            changed['jobs'][job]['with']['components'] = '["runtime-desktop"]'
            with self.assertRaises(ValueError):
                validate(changed, self.publish)

    def test_wrong_omarchy_target_or_args_rejected_in_either_build(self):
        for build in ('Build the native image for scanning', 'Build the complete publication archive'):
            for key, value in (('target', 'default-runtime'), ('build-args', 'RUNTIME_DESKTOP=0\n')):
                changed = copy.deepcopy(self.publish)
                step = next(row for row in changed['jobs']['images']['steps'] if row.get('name') == build)
                step['with'][key] = value
                with self.assertRaises(ValueError):
                    validate(self.ci, changed)


if __name__ == '__main__':
    unittest.main()
