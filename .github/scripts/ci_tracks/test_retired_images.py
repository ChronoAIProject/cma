"""Historical manifests remain readable without selecting retired publications."""
import copy
import json
from pathlib import Path
import shutil
import tempfile
import unittest

from deployable_manifest import CURRENT, RETIRED
from image_inputs import changed_images, image_inputs, recipes
from policy import Workspace
from recipe import resolve
import test_images
from test_policy import metadata
from uncertainty import SelectionIntegrityError


class RetirementTests(unittest.TestCase):
    setUp = test_images.ImageTests.setUp
    write = test_images.ImageTests.write
    pin_recipes = test_images.ImageTests.pin_recipes

    def legacy(self, root):
        path = root / '.github/scripts/deployables.json'
        data = json.loads(path.read_text())
        for name, component in RETIRED.items():
            donor = 'cma-runtime' if component == 'runtime-desktop' else 'cma-agent-manager'
            data[name] = dict(copy.deepcopy(data[donor]), component=component)
        path.write_text(json.dumps(data))
        return data

    def test_seven_to_five_is_not_an_active_recipe_change(self):
        with tempfile.TemporaryDirectory() as directory:
            old = Path(directory) / 'old'
            shutil.copytree(self.root, old)
            legacy = self.legacy(old)
            self.assertEqual(set(recipes(old)), set(CURRENT) | set(RETIRED))
            snapshots = [(old, Workspace(metadata(old), old), legacy),
                         (self.root, self.workspace, self.manifest)]
            self.assertEqual(changed_images(['.github/scripts/deployables.json'], snapshots), [])
            self.assertEqual(changed_images(['api/Cargo.lock'], snapshots), sorted(set(CURRENT) - {'cma-web'}))
            self.assertEqual(changed_images(['web/src/main.tsx'], snapshots), ['cma-web'])
            for component in RETIRED.values():
                with self.assertRaises(ValueError):
                    resolve(old, component)
            self.assertEqual(resolve(old, 'runtime-omarchy')['target'], 'supervised-omarchy')

    def test_seven_to_seven_history_never_resurrects_retired_builds(self):
        self.legacy(self.root)
        snapshot = (self.root, self.workspace, self.manifest)
        self.assertEqual(changed_images(['api/Cargo.lock'], [snapshot, snapshot]),
                         sorted(set(CURRENT) - {'cma-web'}))

    def test_retired_recipe_sources_are_not_current_build_requirements(self):
        legacy = self.legacy(self.root)
        for name in RETIRED:
            legacy[name]['dockerfile'] = 'retired/Dockerfile'
        self.write('.github/scripts/deployables.json', json.dumps(legacy))
        self.assertEqual(set(image_inputs(self.root, self.workspace)), set(CURRENT))

    def test_historical_recipe_change_retains_active_version_selection(self):
        with tempfile.TemporaryDirectory() as directory:
            old = Path(directory) / 'old'
            shutil.copytree(self.root, old)
            legacy = self.legacy(old)
            legacy['cma-runtime-omarchy']['target'] = 'omarchy'
            (old / '.github/scripts/deployables.json').write_text(json.dumps(legacy))
            self.assertEqual(resolve(old, 'runtime-omarchy')['target'], 'omarchy')
            snapshots = [(old, Workspace(metadata(old), old), None),
                         (self.root, self.workspace, None)]
            self.assertEqual(changed_images(['.github/scripts/deployables.json'], snapshots),
                             ['cma-runtime-omarchy'])

    def test_partial_or_rebound_historical_manifests_refuse(self):
        legacy = self.legacy(self.root)
        candidates = []
        for name in CURRENT:
            partial = copy.deepcopy(legacy)
            del partial[name]
            candidates.append(partial)
        partial = copy.deepcopy(legacy)
        del partial['cma-image-manager']
        candidates.append(partial)
        rebound = copy.deepcopy(legacy)
        rebound['cma-runtime']['component'] = 'runtime-desktop'
        candidates.append(rebound)
        candidates.append(dict(legacy, **{'cma-other': legacy['cma-web']}))
        for data in candidates:
            self.write('.github/scripts/deployables.json', json.dumps(data))
            with self.assertRaises(SelectionIntegrityError):
                recipes(self.root)


if __name__ == '__main__':
    unittest.main()
