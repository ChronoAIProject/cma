"""Qualified peers deploy without weakening scan, source or group authority."""
import contextlib
import fnmatch
import io
import json
import os
from pathlib import Path
import subprocess
import tempfile
import unittest
from unittest.mock import Mock, patch

from staging_cd_admission_report import write_admission
from staging_cd_common import Refusal
from staging_cd_compatibility import ContractPolicy, GROUP
from staging_cd_component_admission import automatic_admission, verified_resume
from staging_cd_history import DevelopHistory
from staging_cd_invoke import deploy
from staging_cd_deploy_result_tests import fixture_result
import staging_cd_invoke_tests
from staging_cd_promotion import proof
from staging_cd_receipts import canonical
from staging_cd_receipts_tests import fixture_receipt, fixture_runtime_receipt, fixture_tag
from staging_cd_set import private_document, verify_set
from staging_cd_versions import DEPLOYABLES
from staging_cd_watermark import require_current_versions
import staging_cd_set_report as report


class ComponentAdmissionTests(unittest.TestCase):
    def setUp(self):
        self.receipts = {key: fixture_receipt(component=key) for key in ('am', 'eg', 'web')}
        self.receipts.update({key: fixture_runtime_receipt(component=key) for key in ('runtime', 'runtime-omarchy')})
        self.api, self.registry = Mock(), Mock()
        self.api.history.return_value = DevelopHistory('f' * 40, ['f' * 40, 'a' * 40])
        self.api.tags.side_effect = lambda key: [fixture_tag(self.receipts[key])]
        self.registry.exists.return_value = True
        self.registry.manifest.return_value = ('sha256:' + 'd' * 64, {})
        self.policy = ContractPolicy(self.api.history())
        self.policy.transition = Mock(return_value=False)
        self.policy.signature = Mock(return_value=('contract', 'bootstrap'))

    def matrix(self, keys):
        return {'include': [{'component': key, 'tasks': canonical([
            {'component': key, 'mode': 'resume', 'receipt': self.receipts[key]}])} for key in keys]}

    def admit(self, keys):
        return automatic_admission(self.matrix(keys), self.api, self.registry, policy=self.policy)

    def test_queued_resume_requires_current_receipts_for_the_whole_admitted_set(self):
        keys = ["web", "eg"]
        matrix = self.matrix(keys)
        value, failures = self.admit(keys)
        components = private_document(value)["components"]
        confirmed = {"writer": "f" * 40, "stamp": proof(components)}
        evidence = dict.fromkeys(keys, confirmed)
        read = Mock(return_value=evidence)
        self.assertTrue(verified_resume(matrix, value, failures, self.api, read))
        del evidence["eg"]
        self.assertFalse(verified_resume(matrix, value, failures, self.api, read))
        read.return_value = {key: {"writer": "f" * 40, "stamp": proof({key: release})}
                             for key, release in components.items()}
        self.assertFalse(verified_resume(matrix, value, failures, self.api, read))
        wrong = dict(components, eg=dict(components["eg"], image="sha256:" + "a" * 64))
        read.return_value = dict.fromkeys(keys, {"writer": "f" * 40, "stamp": proof(wrong)})
        self.assertFalse(verified_resume(matrix, value, failures, self.api, read))
        read.side_effect = OSError()
        self.assertFalse(verified_resume(matrix, value, failures, self.api, read))

    def test_new_publication_or_partial_admission_never_uses_an_old_proof(self):
        matrix = self.matrix(["web"])
        value, failures = self.admit(["web"])
        read = Mock()
        reservation = dict(self.receipts["web"]["identity"], schema="cma-version-reservation-v1")
        del reservation["build"]
        matrix["include"][0]["tasks"] = canonical([
            {"mode": "publish", "component": "web", "reservation": reservation}])
        self.assertFalse(verified_resume(matrix, value, failures, self.api, read))
        self.assertFalse(verified_resume(self.matrix(["web"]), value, {"eg": "publication-unqualified"}, self.api, read))
        read.assert_not_called()

    def unavailable(self, *keys):
        self.api.tags.side_effect = lambda key: [] if key in keys else [fixture_tag(self.receipts[key])]

    def execute(self, value, *, check=None, expected=None):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            env = staging_cd_invoke_tests.InvokeTests().environment(root)
            env['CD_RELEASE_SET'] = canonical(value)
            script = root / 'staging/scripts/deploy_set.py'
            script.parent.mkdir(parents=True)
            (script.parent.parent / 'config.yaml').write_text(
                'apps:\n  am: {deploy: enabled}\n  eg: {deploy: enabled}\n  web: {deploy: enabled}\n')
            observed = []
            def run(_script, path, _mode, _started, _env):
                observed.append(json.loads(path.read_text()))
                fixture_result(path)
                return 0
            runner = Mock(side_effect=run)
            with patch("staging_cd_invoke.require_current_versions", side_effect=check) as guard:
                self.assertEqual(deploy(env, Mock(), lambda _: script, runner), 0)
                guard.assert_called_once()
            runner.assert_called_once()
            self.assertEqual(set(observed[0]['components']), expected or set(value['receipts']))
            return observed[0]

    def advertisement(self, *names):
        refs = ['refs/tags/' + receipt['tag'] for receipt in self.receipts.values()]
        refs += ['refs/tags/' + name for name in names]

        def runner(args, **_kwargs):
            lines = ['1' * 40 + '\t' + ref for ref in refs if any(fnmatch.fnmatchcase(ref, p) for p in args)]
            return subprocess.CompletedProcess(args, 0, '\n'.join(lines), '')
        return lambda receipts: require_current_versions(receipts, runner=runner)

    def test_every_lane_with_a_pending_successor_deploys_but_a_higher_final_is_withheld(self):
        keys, apps = sorted(DEPLOYABLES), ['am', 'eg', 'web']
        newer = {key: DEPLOYABLES[key] + '/v' + ('0.10.3' if key == 'runtime' else '0.10.2') for key in keys}
        successors = [tag.replace('/v', '/pending/v') for tag in newer.values()]
        pending = self.advertisement(*successors)
        self.api.require_current_versions.side_effect = pending
        value, failures = self.admit(keys)
        self.assertEqual((set(value['receipts']), failures), (set(keys), {}))
        value, _ = self.admit(apps)
        self.assertEqual(verify_set(value, self.api, self.registry, automatic=True), value)
        self.execute(value, check=pending)
        for key in keys:
            with self.subTest(component=key):
                final = self.advertisement(*successors, newer[key])
                self.api.require_current_versions.side_effect = final
                narrowed, failures = self.admit(keys)
                self.assertEqual(set(narrowed['receipts']), set(keys) - {key})
                self.assertEqual(failures, {key: 'superseded-source'})
                if key in apps:
                    # A final arriving after admission withholds its lane under the lock and at launch.
                    current = set(apps) - {key}
                    self.assertEqual(set(verify_set(value, self.api, self.registry, automatic=True)['receipts']),
                                     current)
                    self.execute(value, check=final, expected=current)

    def test_new_pending_version_withholds_only_its_component(self):
        from staging_cd_watermark import SUPERSEDED
        def current(receipts):
            if "eg" in receipts:
                raise Refusal(SUPERSEDED)
        self.api.require_current_versions.side_effect = current
        value, failures = automatic_admission(self.matrix(["web", "eg"]), self.api, self.registry)
        self.assertEqual(set(value["receipts"]), {"web"})
        self.assertEqual(failures, {"eg": "superseded-source"})
        with self.assertRaisesRegex(Refusal, "superseded"):
            verify_set({"schema": value["schema"], "receipts": {"eg": self.receipts["eg"]}},
                       self.api, self.registry, automatic=True)

    def test_web_publication_success_runtime_scan_failure_deploys_only_web(self):
        self.receipts['runtime']['scan']['complete'] = False
        value, failures = self.admit(['web', 'runtime'])
        self.assertEqual(set(value['receipts']), {'web'})
        self.assertEqual(failures, {'runtime': 'publication-unqualified'})
        self.execute(value)

    def test_am_success_with_eg_failure_deploys_am(self):
        self.unavailable('eg')
        value, failures = self.admit(['am', 'eg'])
        self.assertEqual(set(value['receipts']), {'am'})
        self.assertEqual(failures, {'eg': 'publication-unqualified'})
        self.execute(value)

    def test_legacy_am_without_compatibility_is_withheld_without_blocking_web(self):
        self.receipts['am'].pop('compatibility')
        self.receipts['am']['schema'] = 'cma-published-image-v1'
        value, failures = self.admit(['am', 'web'])
        self.assertEqual(set(value['receipts']), {'web'})
        self.assertEqual(failures, {'am': 'publication-unqualified'})
        self.execute(value)

    def test_contract_transition_failure_withholds_whole_group_but_not_web(self):
        self.policy.transition.return_value = True
        self.unavailable('runtime-omarchy')
        value, failures = self.admit([*sorted(GROUP), 'web'])
        self.assertEqual(set(value['receipts']), {'web'})
        self.assertEqual(set(failures), GROUP)
        self.assertEqual(failures['am'], 'contract-group-incomplete')
        self.assertEqual(failures['runtime-omarchy'], 'publication-unqualified')
        self.execute(value)
        value, failures = self.admit(sorted(GROUP))
        self.assertIsNone(value)
        self.assertEqual(set(failures), GROUP)

    def test_complete_transition_coalesces_am_and_both_runtimes_in_one_call(self):
        self.policy.transition.return_value = True
        value, failures = self.admit(sorted(GROUP))
        self.assertEqual(failures, {})
        self.assertEqual(set(self.execute(value)['components']), GROUP)

    def test_transition_retry_resumes_only_exact_already_finalized_peers(self):
        self.policy.transition.return_value = True
        value, failures = self.admit(['runtime'])
        self.assertEqual(failures, {})
        self.assertEqual(set(value['receipts']), GROUP)
        self.execute(value)
        self.receipts['am'] = fixture_receipt(component='am', source='f' * 40)
        value, failures = self.admit(['runtime'])
        self.assertIsNone(value)
        self.assertEqual(set(failures), GROUP)

    def test_transition_retry_missing_peer_cannot_deploy_successful_group_subset(self):
        self.policy.transition.return_value = True
        self.unavailable('runtime-omarchy')
        value, failures = self.admit(['runtime', 'web'])
        self.assertEqual(set(value['receipts']), {'web'})
        self.assertEqual(set(failures), GROUP)

    def test_all_failed_components_admit_nothing(self):
        self.unavailable(*self.receipts)
        value, failures = self.admit(sorted(self.receipts))
        self.assertIsNone(value)
        self.assertEqual(set(failures), set(self.receipts))

    def test_unknown_contract_analysis_blocks_group_and_has_no_raw_diagnostic(self):
        self.policy.transition.side_effect = ValueError('private-contract-canary')
        value, failures = self.admit(['am', 'web'])
        self.assertEqual(set(value['receipts']), {'web'})
        self.assertEqual(failures, {'am': 'contract-analysis-unavailable'})
        self.assertNotIn('private-contract-canary', canonical(failures))

    def test_malformed_shared_matrix_cannot_become_partial_deployment(self):
        matrix = self.matrix(['web'])
        for malformed in ({}, {'include': [*matrix['include'], *matrix['include']]},
                          {'include': [{'component': 'unknown', 'tasks': '[]'}]},
                          {'include': [{'component': 'web', 'tasks': '[]'}]}):
            with self.subTest(malformed=malformed), self.assertRaises(Refusal):
                automatic_admission(malformed, self.api, self.registry, policy=self.policy)
        self.registry.exists.assert_not_called()

    def test_bad_digest_missing_smoke_or_receipt_cannot_be_admitted(self):
        for defect in ('digest', 'smoke', 'receipt'):
            with self.subTest(defect=defect):
                self.setUp()
                if defect == 'digest':
                    self.registry.manifest.side_effect = lambda repository, version: (
                        'sha256:' + ('f' if repository.endswith('cma-runtime') else 'd') * 64, {})
                elif defect == 'smoke':
                    self.receipts['runtime'].pop('runtime_smoke')
                else:
                    self.api.tags.side_effect = lambda key: [{'private': 'value'}] if key == 'runtime' else [fixture_tag(self.receipts[key])]
                value, failures = self.admit(['web', 'runtime'])
                self.assertEqual(set(value['receipts']), {'web'})
                self.assertIn('runtime', failures)

    def test_failed_tail_cannot_fall_back_to_successful_prefix(self):
        matrix = self.matrix(['web', 'eg'])
        tail = fixture_receipt(previous='0.10.1', version='0.10.2', source='f' * 40)
        tasks = json.loads(matrix['include'][0]['tasks'])
        tasks.append({'component': 'web', 'mode': 'resume', 'receipt': tail})
        matrix['include'][0]['tasks'] = canonical(tasks)
        value, failures = automatic_admission(matrix, self.api, self.registry, policy=self.policy)
        self.assertEqual(set(value['receipts']), {'eg'})
        self.assertEqual(failures, {'web': 'publication-unqualified'})

    def test_superseded_component_is_withheld_before_and_after_lock(self):
        old = self.matrix(['web', 'eg'])
        value, _ = self.admit(['web', 'eg'])
        later = fixture_receipt(previous='0.10.1', version='0.10.2', source='f' * 40)
        self.api.tags.side_effect = lambda key: ([fixture_tag(self.receipts[key]), fixture_tag(later)]
                                               if key == 'web' else [fixture_tag(self.receipts[key])])
        narrowed, failures = automatic_admission(old, self.api, self.registry, policy=self.policy)
        self.assertEqual(set(narrowed['receipts']), {'eg'})
        self.assertEqual(failures, {'web': 'superseded-source'})
        self.assertEqual(verify_set(value, self.api, self.registry, automatic=True), narrowed)

    def test_admission_artifact_binds_attempt_complete_coverage_and_digests(self):
        self.unavailable('runtime')
        value, failures = self.admit(['web', 'runtime'])
        with tempfile.TemporaryDirectory() as directory:
            env = {'RUNNER_TEMP': directory, 'GITHUB_RUN_ID': '12', 'GITHUB_RUN_ATTEMPT': '2', 'GITHUB_SHA': 'a' * 40}
            artifact = write_admission(env, ['web', 'runtime'], value, failures, '2026-01-01T00:00:00Z')
            self.assertEqual(json.loads((Path(directory) / 'public-admission/admission.json').read_text()), artifact)
            self.assertEqual(artifact['release_set']['receipts']['web']['digest'], self.receipts['web']['digest'])
            self.assertEqual((artifact['run_id'], artifact['run_attempt']), (12, 2))
            for selected, failed in ((['web'], failures), (['web', 'runtime'], {}), (['web'], {'web': 'private-value'})):
                with self.assertRaises(Refusal):
                    write_admission(env, selected, value, failed, '2026-01-01T00:00:00Z')

    def test_failed_peer_keeps_final_accounting_red_after_successful_deploy(self):
        self.unavailable('runtime')
        value, failures = self.admit(['web', 'runtime'])
        env = {'CD_RELEASE_SET': canonical(value), 'CD_COMPONENT_FAILURES': canonical(failures),
               'CD_ADMISSION_JOB_RESULT': 'success', 'CD_DEPLOY_JOB_RESULT': 'success',
               'CD_PUBLICATION_JOB_RESULT': 'failure', 'CD_SET_EXPECTED': 'true'}
        with patch.dict(os.environ, env, clear=True), contextlib.redirect_stdout(io.StringIO()) as output:
            self.assertEqual(report.main(), 1)
        row = json.loads(next(line[10:] for line in output.getvalue().splitlines() if line.startswith('CD_RESULT ')))
        self.assertEqual(row['admitted'], ['web'])
        self.assertEqual(row['failed'], failures)
        self.assertIn('FAIL component=runtime reason=publication-unqualified', output.getvalue())
        env.update(CD_COMPONENT_FAILURES='{"runtime":"private-value"}')
        with patch.dict(os.environ, env, clear=True), contextlib.redirect_stdout(io.StringIO()) as output:
            self.assertEqual(report.main(), 1)
        self.assertNotIn('private-value', output.getvalue())


if __name__ == '__main__':
    unittest.main()
