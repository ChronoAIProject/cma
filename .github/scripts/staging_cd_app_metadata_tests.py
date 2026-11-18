"""App checkout scope, immutable commits and private metadata confidentiality."""
import contextlib
import io
import os
import tempfile
import unittest
from pathlib import Path
from unittest.mock import Mock, patch

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
