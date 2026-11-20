"""App checkout scope, immutable commits and private metadata confidentiality."""
import contextlib
import io
import os
import socket
import tempfile
import unittest
import urllib.error
from pathlib import Path
from unittest.mock import Mock, patch

import yaml

import staging_cd_app_metadata as metadata
import staging_cd_checkout as checkout
from staging_cd_common import Refusal
from staging_cd_runtime import run_private


class AppMetadataTests(unittest.TestCase):
    def test_main_is_resolved_once_and_the_immutable_commit_is_then_checked(self):
        original = self.responses()
        read = Mock(side_effect=[original[0], {"ref": "refs/heads/main", "object": {"type": "commit", "sha": "a" * 40}}, original[1]])
        self.assertEqual(metadata.resolve_app_commit("fixture", "main", Mock(), lambda _: read), "a" * 40)
        self.assertEqual(read.call_args_list[1].args[0], f"repos/{metadata.REPOSITORY}/git/ref/heads/main")
        self.assertEqual(read.call_args_list[2].args[0], f"repos/{metadata.REPOSITORY}/git/commits/" + "a" * 40)

    def responses(self):
        return [
            {"total_count": 1, "repositories": [{"full_name": metadata.REPOSITORY, "private": True}]},
            {"sha": "a" * 40, "message": "private fixture subject\ncontinued\n\nprivate body"},
        ]

    def test_single_private_repository_and_immutable_commit_are_required(self):
        masks = []
        read = Mock(side_effect=self.responses())
        self.assertEqual(metadata.resolve_app_commit("fixture", "a" * 40, masks.append, lambda token: read), "a" * 40)
        self.assertEqual([call.args[0] for call in read.call_args_list], [
            "installation/repositories?per_page=100",
            f"repos/{metadata.REPOSITORY}/git/commits/" + "a" * 40,
        ])
        self.assertEqual(masks[0], "fixture")
        self.assertIn("private fixture subject continued", masks[-1])
        self.assertIn("private body", masks[-1])

    def test_deploy_workflow_preserves_diagnostics_and_masks_sensitive_values(self):
        workflow = Path(__file__).resolve().parents[1] / "workflows/staging-deploy.yml"
        steps = yaml.safe_load(workflow.read_text())["jobs"]["deploy"]["steps"]
        prepare = next(step for step in steps if step.get("id") == "private")
        checkout_index = next(i for i, step in enumerate(steps) if step.get("with", {}).get("repository"))
        self.assertLess(steps.index(prepare), checkout_index)
        self.assertEqual(prepare["run"], "python3 -B .github/scripts/staging_cd_checkout.py prepare-app")
        self.assertEqual(prepare["env"]["CD_INFRA_TOKEN"], "${{ steps.infra-token.outputs.token }}")

        repo_patch = patch.object(metadata, "REPOSITORY", "Example/xy")
        repo_patch.start()
        self.addCleanup(repo_patch.stop)
        public_revision, private_tree = "a" * 40, "b" * 40
        installation, commit = self.responses()
        installation["repositories"][0].update(
            id=12345678, name="xy", owner={"login": "ab"}, visibility="private",
            permissions={"pull": True}, language="Go", description="x", status="disabled",
        )
        commit.update(tree={"sha": private_tree}, verification={"verified": True, "reason": "valid"},
                      author={"name": "Li", "email": "who@example.invalid"})
        read = Mock(side_effect=[installation, commit])
        output = io.StringIO()
        with tempfile.TemporaryDirectory() as directory:
            env = {"GITHUB_WORKSPACE": directory, "GITHUB_RUN_ID": "123", "GITHUB_RUN_ATTEMPT": "1",
                   "CD_INFRA_TOKEN": "z", "CD_INFRA_REF": public_revision}
            def resolve(token, revision, masker):
                return metadata.resolve_app_commit(token, revision, masker, lambda _: read)
            with contextlib.redirect_stdout(output):
                self.assertEqual(run_private(lambda mask: checkout.prepare_app(env, mask, resolve), None), 0)

        lines = output.getvalue().splitlines()
        self.assertEqual(lines[-1], "PASS")
        self.assertTrue(all(line.startswith("::add-mask::") for line in lines[:-1]))
        masks = {line[len("::add-mask::"):] for line in lines[:-1]}
        required = {"z", metadata.REPOSITORY, "xy", "ab", "Li", "12345678", "who@example.invalid",
                    private_tree, "private body", "private fixture subject continued"}
        self.assertTrue(required <= masks)
        self.assertTrue(masks.isdisjoint({"id", "sha", "verified", "private", "pull", "valid", "disabled", "Go", "x", public_revision}))
        for diagnostic in ("Staging deployment is unverified", "sha256:", "identity", "precondition=private-image-version"):
            self.assertFalse(any(mask in diagnostic for mask in masks))

    def test_confidential_values_take_precedence_over_length_and_public_vocabulary(self):
        fields = {"name": "id", "owner": {"login": "private"}, "node_id": "q", "api_key": "k",
                  "client_secret": "s", "access_token": "t", "credentials": [{"value": "j"}],
                  "verification": {"signature": "v"},
                  "topics": ["short", "synthetic-private-topic"], "user_view_type": "public", "private": True}
        self.assertEqual(set(metadata.metadata_values(fields)),
                         {"id", "private", "q", "k", "s", "t", "j", "v", "synthetic-private-topic"})

    def test_extra_repository_public_repo_or_wrong_repo_never_resolves_head(self):
        cases = [
            {"total_count": 2, "repositories": []},
            {"total_count": True, "repositories": [{"full_name": metadata.REPOSITORY, "private": True}]},
            {"total_count": 1, "repositories": [{"full_name": metadata.REPOSITORY, "private": False}]},
            {"total_count": 1, "repositories": [{"full_name": "Example/other", "private": True}]},
            {"total_count": 1, "repositories": [None]},
        ]
        for value in cases:
            read = Mock(return_value=value)
            with self.subTest(value=value), self.assertRaises(Refusal):
                metadata.resolve_app_commit("fixture", "a" * 40, Mock(), lambda token: read)
            self.assertEqual(read.call_count, 1)

    def test_malformed_reference_and_commit_fail_closed(self):
        for index, value in [(1, {}), (1, {"sha": "b" * 40, "message": "fixture"}),
                             (1, {"sha": "a" * 40, "message": None})]:
            values = self.responses()
            values[index] = value
            with self.subTest(index=index, value=value), self.assertRaises(Refusal):
                metadata.resolve_app_commit("fixture", "a" * 40, Mock(), lambda token: Mock(side_effect=values))
        with self.assertRaises(Refusal):
            metadata.resolve_app_commit("fixture", "develop", Mock(), Mock())

    def test_http_credential_transport_size_status_and_redirect_boundary(self):
        response = Mock(status=200)
        response.read.return_value = b'{"fixture":true}'
        context = contextlib.nullcontext(response)
        opener = Mock()
        opener.open.return_value = context
        with patch.object(metadata.urllib.request, "build_opener", return_value=opener):
            read = metadata.github_reader("fixture")
            self.assertEqual(read("installation/repositories?per_page=100"), {"fixture": True})
            request = opener.open.call_args.args[0]
            self.assertEqual(request.get_header("Authorization"), "Bearer fixture")
            self.assertTrue(request.full_url.startswith("https://api.github.com/"))
            self.assertEqual(opener.open.call_args.kwargs["timeout"], 30)
            for status, body in [(403, b"private"), (200, b"x" * (1024 * 1024 + 1)),
                                 (200, b"[]"), (200, b"invalid")]:
                response.status, response.read.return_value = status, body
                with self.subTest(status=status, size=len(body)), self.assertRaises((Refusal, ValueError)):
                    read("installation/repositories?per_page=100")
        for token in (None, "", "a b", "fixture\n"):
            with self.subTest(token=token), self.assertRaises(Refusal):
                metadata.github_reader(token)
        self.assertIsNone(metadata.NoRedirect().redirect_request(None, None, 302, "", {}, "https://example.invalid"))

    def test_transport_failures_repeat_and_refusals_do_not(self):
        response = Mock(status=200)
        response.read.return_value = b'{"fixture":true}'
        for failure, calls in ((urllib.error.URLError(socket.gaierror(-3, "private")), 2),
                               (urllib.error.HTTPError("https://api.github.com", 503, "private", {}, None), 2),
                               (urllib.error.HTTPError("https://api.github.com", 401, "private", {}, None), 1)):
            opener = Mock()
            opener.open.side_effect = [failure, contextlib.nullcontext(response)]
            with self.subTest(failure=failure), \
                    patch.object(metadata.urllib.request, "build_opener", return_value=opener):
                read = metadata.github_reader("fixture", retry={"sleep": lambda _seconds: None})
                if calls == 2:
                    self.assertEqual(read("installation/repositories?per_page=100"), {"fixture": True})
                else:
                    with self.assertRaises(urllib.error.HTTPError):
                        read("installation/repositories?per_page=100")
            self.assertEqual(opener.open.call_count, calls)

    def test_prepare_app_keeps_metadata_private_and_requires_configured_sha(self):
        with tempfile.TemporaryDirectory() as directory:
            env = {"GITHUB_WORKSPACE": directory, "GITHUB_RUN_ID": "123", "GITHUB_RUN_ATTEMPT": "1",
                   "CD_INFRA_TOKEN": "fixture", "CD_INFRA_REF": "a" * 40}
            def resolve(token, reference, masker):
                self.assertEqual(token, "fixture")
                self.assertEqual(reference, "a" * 40)
                print("private response")
                os.write(2, b"private native output")
                return "a" * 40
            output = io.StringIO()
            with contextlib.redirect_stdout(output):
                self.assertEqual(run_private(lambda masker: checkout.prepare_app(env, masker, resolve), None), 0)
            public = output.getvalue()
            self.assertNotIn("private response", public)
            self.assertTrue(public.endswith("PASS\n"))
            for field, value in [("CD_INFRA_REF", "develop"), ("GITHUB_RUN_ID", "0")]:
                with self.subTest(field=field), self.assertRaises(Refusal):
                    checkout.prepare_app(dict(env, **{field: value}), Mock(), Mock())
            with self.assertRaises(Refusal):
                checkout.prepare_app(env, Mock(), Mock(return_value="main"))
            self.assertEqual(list(Path(directory).iterdir()), [])


if __name__ == "__main__":
    unittest.main()
