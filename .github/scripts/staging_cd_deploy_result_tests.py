"""A partial deployment cannot be called fully deployed or leak private fields."""
import copy
import contextlib
import io
import json
import os
from pathlib import Path
import tempfile
import unittest
from unittest.mock import Mock, patch

from staging_cd_common import Refusal
from staging_cd_deploy_result import emit, read, report, retain, validate
from staging_cd_receipts import canonical, sha256
from staging_cd_receipts_tests import fixture_receipt
from staging_cd_set import private_document, release_set


def fixture_result(path, withheld=()):
    request = json.loads(path.read_text())
    result = {"schema": "cma-private-deployment-result-v1", "request": sha256(request),
              "code": 50 if withheld else 0, "components": {key: {
                  "state": "withheld" if key in withheld else "controller-verified",
                  "reason": "deployed-compatibility-unqualified" if key in withheld else "none",
                  "image": row["image"], "source_sha": row["source_sha"]}
                  for key, row in request["components"].items()}}
    target = path.with_name('component-result.json')
    target.write_text(canonical(result))
    target.chmod(0o600)
    return result


class DeployResultTests(unittest.TestCase):
    def test_execute_late_success_packet_is_a_budget_failure_even_during_grace(self):
        import staging_cd_invoke as invoke
        from staging_cd_invoke_tests import InvokeTests
        from staging_cd_clock import timestamp

        for timeout in (False, True):
            with self.subTest(timeout=timeout), tempfile.TemporaryDirectory() as directory:
                root = Path(directory)
                env = InvokeTests().environment(root)
                env["CD_STARTED_AT"] = timestamp(1000)
                clock = [1020]
                summary = root / "summary"
                env["GITHUB_STEP_SUMMARY"] = str(summary)
                deploy = invoke.deploy

                def process(args, *_args, **_kwargs):
                    release = Path(args[args.index("--release-set") + 1])
                    fixture_result(release)
                    clock[0] = 1991
                    if timeout:
                        raise invoke.ProcessTimeout("fixture", 970, 0)
                    return 0

                def operation(values, masker, **callbacks):
                    return deploy(values, masker, lambda _: root / "staging/scripts/deploy.py",
                                  policy_reader=lambda *a, **kw: ["web"], **callbacks)

                output = io.StringIO()
                with patch.dict(os.environ, env, clear=True), patch.object(invoke, "run_process", side_effect=process), \
                        patch("staging_cd_clock.time.time", side_effect=lambda: clock[0]), \
                        patch.object(invoke, "require_current_versions") as current, \
                        patch.object(invoke, "deploy", side_effect=operation), contextlib.redirect_stdout(output):
                    self.assertEqual(invoke.main(), 20)
                current.assert_called_once_with({"web": fixture_receipt()})
                for text in (output.getvalue(), summary.read_text()):
                    self.assertIn("FAIL verification=phase-budget recovery=unreported", text)
                    self.assertNotIn("FAIL verification=result", text)
                    self.assertNotIn("PASS_PENDING", text)
                self.assertFalse((root / "public-deployment").exists())

    def test_late_controller_success_cannot_export_verified_evidence(self):
        import staging_cd_invoke as invoke
        from staging_cd_invoke_tests import InvokeTests
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            env = InvokeTests().environment(root)
            env['CD_RELEASE_SET'] = canonical(release_set({'am': fixture_receipt(component='am')}))
            def runner(_script, path, *_args):
                fixture_result(path)
                return 0
            with patch.object(invoke, 'elapsed', return_value=991), patch.object(invoke, 'require_current_versions'):
                self.assertEqual(invoke.deploy(env, Mock(), lambda _: root / 'deploy_set.py', runner,
                                 policy_reader=lambda *_args, **_kwargs: ['am']), 20)
            self.assertFalse(Path(env['GITHUB_OUTPUT']).exists())
            self.assertFalse((root / 'public-deployment').exists())

    def test_partial_private_execution_fails_run_and_reports_each_withheld_component(self):
        import staging_cd_invoke as invoke
        from staging_cd_invoke_tests import InvokeTests
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            env = InvokeTests().environment(root)
            env['GITHUB_OUTPUT'] = str(root / 'step-output')
            env['CD_RELEASE_SET'] = canonical(release_set({key: fixture_receipt(component=key) for key in ('am', 'web')}))
            original = invoke.deploy
            def runner(_script, path, *_args):
                fixture_result(path, {'am'})
                print('private-output-canary')
                return 50
            def deploy(environment, masker, **kwargs):
                return original(environment, masker, lambda _: root / 'deploy_set.py', runner,
                                policy_reader=lambda *_args, **_kwargs: ['am', 'web'], **kwargs)
            output = io.StringIO()
            with patch.dict(os.environ, env, clear=True), patch.object(invoke, 'deploy', side_effect=deploy), \
                    patch.object(invoke, 'require_current_versions'), contextlib.redirect_stdout(output):
                self.assertEqual(invoke.main(), 50)
            self.assertIn('FAIL component=am reason=deployed-compatibility-unqualified', output.getvalue())
            self.assertNotIn('private-output-canary', output.getvalue())
            artifact = json.loads((root / 'public-deployment/deployment.json').read_text())
            self.assertEqual(artifact['components']['web']['state'], 'controller-verified')
            self.assertEqual(artifact['components']['am']['state'], 'withheld')
            outputs = dict(line.split('=', 1) for line in (root / 'step-output').read_text().splitlines())
            transported = outputs['component_result']
            self.assertRegex(outputs['controller_proof'], r'^web [0-9a-f]{64}$')
            self.assertNotIn('private-output-canary', transported)
            report_root = root / 'reporter'
            report_root.mkdir()
            evidence = report({**env, 'RUNNER_TEMP': str(report_root), 'CD_COMPONENT_RESULT': transported})
            self.assertEqual(evidence, artifact)

    def test_partial_result_is_bound_sanitized_and_explicitly_failed(self):
        request = private_document(release_set({key: fixture_receipt(component=key) for key in ('am', 'web')}))
        with tempfile.TemporaryDirectory() as directory:
            path = Path(directory) / 'release-set.json'
            path.write_text(canonical(request))
            result = fixture_result(path, {'am'})
            self.assertEqual(read(path.with_name('component-result.json'), request, 50), result)
            output = retain(result, {'RUNNER_TEMP': directory, 'GITHUB_RUN_ID': '42', 'GITHUB_RUN_ATTEMPT': '1'})
            self.assertEqual(output['components']['am']['state'], 'withheld')
            self.assertEqual(output['components']['web']['state'], 'controller-verified')
            for key, value in [('request', 'f' * 64), ('code', 0), ('unexpected', 'private-value')]:
                with self.subTest(key=key), self.assertRaises(Refusal):
                    validate(dict(result, **{key: value}), request, 50)
            for key, value in [('image', 'private-value'), ('reason', 'private-value'), ('source_sha', 'b' * 40)]:
                changed = copy.deepcopy(result)
                changed['components']['am'][key] = value
                with self.subTest(key=key), self.assertRaises(Refusal):
                    validate(changed, request, 50)

    def test_reporting_revalidates_exact_request_without_credentials(self):
        admitted = release_set({key: fixture_receipt(component=key) for key in ('am', 'web', 'eg')})
        selected = release_set({key: admitted['receipts'][key] for key in ('am', 'web')})
        request = private_document(selected)
        with tempfile.TemporaryDirectory() as directory:
            path = Path(directory) / 'release-set.json'
            path.write_text(canonical(request))
            result = fixture_result(path, {'am'})
            env = {'RUNNER_TEMP': directory, 'GITHUB_RUN_ID': '42', 'GITHUB_RUN_ATTEMPT': '1',
                   'GITHUB_OUTPUT': str(Path(directory) / 'step-output'), 'CD_RELEASE_SET': canonical(admitted)}
            emit(result, request, 50, env)
            self.assertEqual(report(dict(env, CD_COMPONENT_RESULT=canonical(result)))['code'], 50)
            self.assertIsNone(report(dict(env, CD_COMPONENT_RESULT='')))
            for field, value in [('request', 'f' * 64), ('code', 0), ('unexpected', 'private-value'),
                                 ('components', {}), ('components', {'unknown': {}})]:
                bad = dict(result, **{field: value})
                with self.subTest(field=field), self.assertRaises(Refusal):
                    report(dict(env, CD_COMPONENT_RESULT=canonical(bad)))
            for field, value in [('source_sha', 'f' * 40), ('image', 'private-value'), ('reason', 'private-value')]:
                bad = copy.deepcopy(result)
                bad['components']['am'][field] = value
                with self.subTest(field=field), self.assertRaises(Refusal):
                    report(dict(env, CD_COMPONENT_RESULT=canonical(bad)))
            for payload in (' ' * 16385, canonical(result).replace('"code":', '"code":50,"code":', 1)):
                with self.assertRaises(Refusal):
                    report(dict(env, CD_COMPONENT_RESULT=payload))

    def test_missing_unsafe_or_contradictory_results_cannot_prove_success(self):
        request = private_document(release_set({'am': fixture_receipt(component='am')}))
        with tempfile.TemporaryDirectory() as directory:
            path = Path(directory) / 'release-set.json'
            path.write_text(canonical(request))
            target = path.with_name('component-result.json')
            with self.assertRaises(Refusal):
                read(target, request, 0)
            self.assertIsNone(read(target, request, 30))
            result = fixture_result(path)
            target.chmod(0o644)
            with self.assertRaises(Refusal):
                read(target, request, 0)
            target.unlink()
            os.symlink(path, target)
            with self.assertRaises(Refusal):
                read(target, request, 0)
            result['code'] = 50
            with self.assertRaises(Refusal):
                validate(result, request, 50)


if __name__ == '__main__':
    unittest.main()
