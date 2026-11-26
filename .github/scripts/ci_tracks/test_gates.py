import copy
import json
import unittest

from children import evaluate as child_result
from gate import EXPECTED, evaluate
from version_label import deployables, selected_upgrade


def needs(track, selected=True):
    tracks = dict(repo=True, am=selected, eg=selected, web=selected)
    outputs = {key: json.dumps(value) for key, value in tracks.items()}
    outputs['approval_contract'] = json.dumps(selected)
    outputs['two_am_features'] = json.dumps(selected)
    outputs.update(test_tracks=json.dumps(tracks), deployables='[]', am_packages='["cma"]' if selected else '[]', eg_packages='["cmaeg"]' if selected else '[]')
    result = {job: {'result': 'success' if selected or track == 'repo' else 'skipped'} for job in EXPECTED[track]}
    result['changes'] = {'result': 'success', 'outputs': outputs}
    return result


class GateTests(unittest.TestCase):
    def test_every_track_requires_success(self):
        for track, jobs in EXPECTED.items():
            self.assertEqual(evaluate(track, needs(track)), 'passed')
            for job in jobs | {'changes'}:
                for status in ['failure', 'cancelled', 'skipped', None]:
                    data = needs(track)
                    data[job]['result'] = status
                    with self.subTest(track=track, job=job, status=status), self.assertRaises(ValueError):
                        evaluate(track, data)
                data = needs(track)
                del data[job]
                with self.assertRaises(ValueError):
                    evaluate(track, data)

    def test_legitimate_path_skip_with_shared_success(self):
        for track in ['am', 'eg', 'web']:
            data = needs(track, False)
            for job in {'rust-test-build', 'runtime-images', 'omarchy'} & set(data):
                data[job]['result'] = 'success'
            self.assertEqual(evaluate(track, data), 'skipped-by-path')
            data[track]['result'] = 'failure'
            with self.assertRaises(ValueError):
                evaluate(track, data)

    def test_malformed_outputs_never_skip(self):
        for key, value in [('am', ''), ('eg', 'False'), ('repo', 'false'), ('test_tracks', '{}'), ('deployables', 'null'), ('am_packages', 'null'), ('eg_packages', '["cmaeg"]'), ('test_tracks', '{"repo":1,"am":false,"eg":false,"web":false}'), ('deployables', '["private-target"]')]:
            data = needs('am', False)
            data['changes']['outputs'][key] = value
            with self.assertRaises(ValueError):
                evaluate('am', data)

    def test_split_am_requires_runtime_images_for_eg_only_changes(self):
        data = needs('am', False)
        outputs = data['changes']['outputs']
        tracks = json.loads(outputs['test_tracks'])
        tracks['eg'] = True
        outputs.update(eg='true', eg_packages='["cmaeg"]', test_tracks=json.dumps(tracks))
        for job in ('runtime-images', 'omarchy'):
            data[job]['result'] = 'success'
        self.assertEqual(evaluate('am', data, split=True), 'passed')
        for job in ('runtime-images', 'omarchy', 'changes'):
            for status in ('skipped', 'failure', 'cancelled', None):
                mutated = copy.deepcopy(data)
                mutated[job]['result'] = status
                with self.subTest(job=job, status=status), self.assertRaises(ValueError):
                    evaluate('am', mutated, split=True)

    def test_approval_contract_requires_only_test_archive_for_web_only_changes(self):
        data = needs('am', False)
        outputs = data['changes']['outputs']
        tracks = json.loads(outputs['test_tracks'])
        tracks['web'] = True
        outputs.update(web='true', approval_contract='true', test_tracks=json.dumps(tracks))
        data['rust-test-build']['result'] = 'success'
        data['approval-contract']['result'] = 'success'
        for split in (False, True):
            self.assertEqual(evaluate('am', data, split=split), 'passed')
            for job in ('rust-test-build', 'approval-contract'):
                for status in ('failure', 'cancelled', 'skipped'):
                    mutated = copy.deepcopy(data)
                    mutated[job]['result'] = status
                    with self.subTest(split=split, job=job, status=status), self.assertRaises(ValueError):
                        evaluate('am', mutated, split=split)
        data['changes']['outputs'].pop('approval_contract')
        with self.assertRaises(ValueError):
            evaluate('am', data, split=True)

    def test_two_manager_fixture_is_required_only_when_selected(self):
        data = needs('am')
        data['changes']['outputs']['two_am_features'] = 'false'
        with self.assertRaisesRegex(ValueError, 'fixture did not reach'):
            evaluate('am', data, split=True)
        data['two-am-features']['result'] = 'skipped'
        self.assertEqual(evaluate('am', data, split=True), 'passed')
        del data['changes']['outputs']['two_am_features']
        with self.assertRaisesRegex(ValueError, 'fixture selection'):
            evaluate('am', data, split=True)

    def test_split_eg_expects_no_runtime_jobs(self):
        for selected in (True, False):
            data = needs('eg', selected)
            for job in ('runtime-images', 'omarchy'):
                del data[job]
            self.assertEqual(evaluate('eg', data, split=True),
                             'passed' if selected else 'skipped-by-path')
            data['eg']['result'] = 'cancelled'
            with self.assertRaises(ValueError):
                evaluate('eg', data, split=True)

    def test_reusable_result_requires_all_children(self):
        for track in ['am', 'eg']:
            data = {job: {'result': 'success'} for job in ['lint', 'tests', 'timing', 'mongo', 'isolation']}
            if track == 'eg':
                data['isolation']['result'] = 'skipped'
            child_result(track, data)
            for job in data:
                bad = copy.deepcopy(data)
                bad[job]['result'] = 'cancelled'
                with self.assertRaises(ValueError):
                    child_result(track, bad)

    def test_labels_required_independently_of_deployables(self):
        def event(*labels):
            return {'pull_request': {'labels': [{'name': name} for name in labels]}}
        for images in [[], ['cma-web']]:
            for bump in ['major', 'minor', 'patch']:
                with self.subTest(images=images, bump=bump):
                    self.assertEqual(selected_upgrade(event('unrelated', 'version-upgrade:' + bump), images), bump)
            for labels in [[], ['unrelated'], ['version-upgrade:other'],
                           ['version-upgrade:major', 'version-upgrade:minor'],
                           ['version-upgrade:patch', 'version-upgrade:other'],
                           ['version-upgrade:patch', 'version-upgrade:patch']]:
                with self.subTest(images=images, labels=labels), self.assertRaises(ValueError):
                    selected_upgrade(event(*labels), images)
            for labels in [None, {}, ['version-upgrade:patch'], [{'name': None}], [{}]]:
                with self.subTest(images=images, malformed=labels), self.assertRaises(ValueError):
                    selected_upgrade({'pull_request': {'labels': labels}}, images)
        for bad in [None, {}, ['cma-web', 'cma-web'], ['unknown'], ['cma-web', 'cma-runtime']]:
            with self.assertRaises(ValueError):
                deployables(bad)


if __name__ == '__main__':
    unittest.main()
