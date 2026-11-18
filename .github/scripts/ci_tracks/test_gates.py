import copy
import json
import unittest

from gate import EXPECTED, evaluate
from version_label import deployables, selected_upgrade


def needs(track, selected=True):
    tracks = dict(repo=True, am=selected, eg=selected, web=selected)
    outputs = {key: json.dumps(value) for key, value in tracks.items()}
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

    def test_legitimate_path_skip_requires_every_own_task_skipped(self):
        for track in ['am', 'eg', 'web']:
            data = needs(track, False)
            self.assertEqual(evaluate(track, data), 'skipped-by-path')
            for job in EXPECTED[track]:
                for status in ['failure', 'cancelled', 'success', None]:
                    changed = copy.deepcopy(data)
                    changed[job]['result'] = status
                    with self.subTest(track=track,job=job,status=status), self.assertRaises(ValueError):
                        evaluate(track, changed)

    def test_gateway_only_change_still_requires_am_runtime_checks(self):
        data = needs('am', False)
        data['changes']['outputs'].update(eg='true', eg_packages='["cmaeg"]',
            test_tracks=json.dumps(dict(repo=True, am=False, eg=True, web=False)))
        for job in ['runtime', 'omarchy']:
            data[job]['result'] = 'success'
        self.assertEqual(evaluate('am', data), 'passed')
        for job in ['runtime', 'omarchy']:
            for status in ['failure', 'cancelled', 'skipped', None]:
                changed = copy.deepcopy(data)
                changed[job]['result'] = status
                with self.subTest(job=job,status=status), self.assertRaises(ValueError):
                    evaluate('am', changed)

    def test_malformed_outputs_never_skip(self):
        for key, value in [('am', ''), ('eg', 'False'), ('repo', 'false'), ('test_tracks', '{}'), ('deployables', 'null'), ('am_packages', 'null'), ('eg_packages', '["cmaeg"]'), ('test_tracks', '{"repo":1,"am":false,"eg":false,"web":false}'), ('deployables', '["private-target"]')]:
            data = needs('am', False)
            data['changes']['outputs'][key] = value
            with self.assertRaises(ValueError):
                evaluate('am', data)

    def test_labels_follow_deployables(self):
        def event(*labels):
            return {'pull_request': {'labels': [{'name': name} for name in labels]}}
        self.assertEqual(selected_upgrade(event(), []), 'not-required')
        self.assertEqual(selected_upgrade(event('version-upgrade:major', 'version-upgrade:minor'), []), 'ignored')
        for bump in ['major', 'minor', 'patch']:
            self.assertEqual(selected_upgrade(event('unrelated', 'version-upgrade:' + bump), ['cma-web']), bump)
        for labels in [[], ['version-upgrade:other'], ['version-upgrade:major', 'version-upgrade:minor']]:
            with self.assertRaises(ValueError):
                selected_upgrade(event(*labels), ['cma-web'])
        for bad in [None, {}, ['cma-web', 'cma-web'], ['unknown'], ['cma-web', 'cma-runtime']]:
            with self.assertRaises(ValueError):
                deployables(bad)


if __name__ == '__main__':
    unittest.main()
