"""All current publication paths use the same five images; only staging builds them."""
import copy
import json
from pathlib import Path
import runpy
import unittest

import yaml

from deployables import registry
from release_promotion import PUBLISHED

ROOT = Path(__file__).resolve().parents[1]
COMPONENTS = ['api', 'web', 'runtime', 'cmaeg', 'runtime-omarchy']


def validate(ci, publish):
    jobs = ci['jobs']
    grouped = {key: json.loads(jobs[key]['with']['components'])
               for key in ('am-images', 'eg-images', 'web-images', 'runtime-images')}
    if grouped != {'am-images': ['api'], 'eg-images': ['cmaeg'], 'web-images': ['web'], 'runtime-images': ['runtime']}:
        raise ValueError('current CI image matrix differs')
    if jobs['omarchy']['uses'] != './.github/workflows/ci-omarchy.yml':
        raise ValueError('Omarchy image qualification missing')
    if sorted(PUBLISHED.values()) != sorted(COMPONENTS):
        raise ValueError('current publication set differs')
    # A release assigns its tags to the staging-verified digests; only staging builds.
    for job in publish['jobs'].values():
        if any(step.get('uses', '').startswith('docker/build-push-action@') for step in job.get('steps', [])):
            raise ValueError('release must promote staging-verified images, never rebuild')
    jobs = publish['jobs']
    select, promote = jobs['select']['steps'][-1], jobs['images']['steps'][-1]
    if select.get('run') != 'python3 -B .github/scripts/release_promotion.py select' \
            or promote.get('run') != 'python3 -B .github/scripts/release_promotion.py promote' \
            or promote['env']['RELEASE_IMAGES'] != '${{ needs.select.outputs.images }}':
        raise ValueError('release tags must name the selected staging-verified digests')
    # No tag is assigned before the runtime evidence passed, and nothing is released before the tags.
    if 'runtime-image-gate' not in jobs['images']['needs'] or 'images' not in jobs['release']['needs'] \
            or 'images' in jobs['runtime-image-gate']['needs']:
        raise ValueError('tags must follow the runtime evidence and precede the release')


class PublicationScopeTests(unittest.TestCase):
    def setUp(self):
        self.ci = yaml.safe_load((ROOT / 'workflows/ci.yml').read_text())
        self.publish = yaml.safe_load((ROOT / 'workflows/publish.yml').read_text())

    def test_all_paths_select_exactly_five_and_build_real_omarchy(self):
        validate(self.ci, self.publish)
        self.assertEqual({recipe['component'] for recipe in registry().values()}, set(COMPONENTS))
        recipe = registry()['cma-runtime-omarchy']
        self.assertEqual(recipe['target'], 'supervised-omarchy')
        self.assertEqual(recipe['build_args'], {'RUNTIME_DESKTOP': '1', 'OMARCHY_CMA_BASE': 'runtime-tools'})
        workflow = yaml.safe_load((ROOT / 'workflows/ci-omarchy.yml').read_text())
        build = next(step for step in workflow['jobs']['omarchy']['steps'] if step.get('id') == 'build')
        self.assertEqual(build['with']['target'], recipe['target'])

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
        for job in ('am-images', 'runtime-images'):
            changed = copy.deepcopy(self.ci)
            changed['jobs'][job]['with']['components'] = '["runtime-desktop"]'
            with self.assertRaises(ValueError):
                validate(changed, self.publish)

    def test_a_rebuild_or_unselected_tag_target_in_the_release_is_rejected(self):
        changed = copy.deepcopy(self.publish)
        changed['jobs']['images']['steps'].insert(1, {'uses': 'docker/build-push-action@' + 'a' * 40})
        with self.assertRaises(ValueError):
            validate(self.ci, changed)
        changed = copy.deepcopy(self.publish)
        changed['jobs']['images']['steps'][-1]['env']['RELEASE_IMAGES'] = '{}'
        with self.assertRaises(ValueError):
            validate(self.ci, changed)
        changed = copy.deepcopy(self.publish)
        changed['jobs']['images']['needs'] = ['plan', 'select']
        with self.assertRaises(ValueError):
            validate(self.ci, changed)

if __name__ == '__main__':
    unittest.main()
