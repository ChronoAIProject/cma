"""Public digest verification never converts transport or auth errors to absence."""
import contextlib
import hashlib
import http.client
import json
import unittest
import urllib.error
from unittest.mock import Mock

from staging_cd_common import Refusal
from staging_cd_registry import Registry, NoRedirect

NO_WAIT = {"sleep": lambda _seconds: None}


class RegistryTests(unittest.TestCase):
    def response(self, value, headers=None):
        data = value if isinstance(value, bytes) else json.dumps(value).encode()
        response = Mock(headers={} if headers is None else headers)
        response.read.return_value = data
        return contextlib.nullcontext(response)

    def test_immutable_manifest_digest_and_platform_shape_are_verified(self):
        manifest = {"schemaVersion": 2, "mediaType": "application/vnd.oci.image.manifest.v1+json",
                    "config": {"digest": "sha256:" + "a" * 64}, "layers": []}
        raw = json.dumps(manifest).encode()
        digest = "sha256:" + hashlib.sha256(raw).hexdigest()
        opener = Mock()
        opener.open.side_effect = [self.response({"token": "fixture"}), self.response(raw, {"Docker-Content-Digest": digest})]
        registry = Registry(opener)
        self.assertEqual(registry.manifest("ghcr.io/chronoaiproject/cma-web", digest), (digest, manifest))
        request = opener.open.call_args.args[0]
        self.assertEqual(request.get_header("Authorization"), "Bearer fixture")
        self.assertTrue(request.full_url.endswith(digest))
        for bad in ("latest", "v0.10.1", "0.10.1\n"):
            with self.subTest(bad=bad), self.assertRaises(Refusal):
                registry.manifest("ghcr.io/chronoaiproject/cma-web", bad)
        with self.assertRaises(Refusal):
            registry.token("ghcr.io/example/other")

    def test_only_explicit_manifest_404_is_absent(self):
        for status in (401, 403, 404, 429, 500):
            opener = Mock()
            error = urllib.error.HTTPError("https://ghcr.io/fixture", status, "private", {}, None)
            opener.open.side_effect = [self.response({"token": "fixture"}), error, error, error]
            registry = Registry(opener, retry=NO_WAIT)
            if status == 404:
                self.assertIsNone(registry.manifest("ghcr.io/chronoaiproject/cma-web", "0.10.1", absent=True))
            else:
                with self.subTest(status=status), self.assertRaises(Refusal):
                    registry.manifest("ghcr.io/chronoaiproject/cma-web", "0.10.1", absent=True)
            # Throttling and server errors repeat within the bound; nothing becomes absence.
            self.assertEqual(opener.open.call_count, 4 if status in (429, 500) else 2)
        opener.open.side_effect = urllib.error.URLError("private")
        with self.assertRaises(Refusal):
            registry.token("ghcr.io/chronoaiproject/cma-web")

    def test_runtime_pointer_reads_are_limited_to_the_two_runtime_repositories(self):
        repository = "ghcr.io/chronoaiproject/cma-runtime"
        opener = Mock()
        registry = Registry(opener, retry=NO_WAIT)
        for unsupported_repository, environment in ((repository, "sandbox"),
                                                    ("ghcr.io/chronoaiproject/cma-web", "staging")):
            with self.subTest(repository=unsupported_repository, environment=environment), self.assertRaises(Refusal):
                registry.runtime_pointer(unsupported_repository, environment)
        with self.assertRaises(Refusal):
            registry.manifest(repository, "staging-current")
        opener.open.assert_not_called()

        missing = urllib.error.HTTPError("https://ghcr.io/fixture", 404, "private", {}, None)
        opener.open.side_effect = [self.response({"token": "fixture"}), missing]
        self.assertIsNone(registry.runtime_pointer(repository, "staging"))
        self.assertTrue(opener.open.call_args.args[0].full_url.endswith("/manifests/staging-current"))

    def test_runtime_pointer_write_copies_the_exact_manifest_to_the_environment_tag(self):
        repository = "ghcr.io/chronoaiproject/cma-runtime"
        manifest = {"schemaVersion": 2, "mediaType": "application/vnd.oci.image.manifest.v1+json",
                    "config": {"digest": "sha256:" + "a" * 64}, "layers": []}
        raw = json.dumps(manifest, indent=2).encode()
        digest = "sha256:" + hashlib.sha256(raw).hexdigest()
        opener = Mock()
        registry = Registry(opener, credential="fixture", actor="writer")
        confirmed = Mock(status=201, headers={"Docker-Content-Digest": digest})
        opener.open.side_effect = [self.response({"token": "pull-fixture"}),
                                   self.response(raw, {"Docker-Content-Digest": digest}),
                                   self.response({"token": "push-fixture"}),
                                   contextlib.nullcontext(confirmed)]
        prepared = registry.prepare_runtime_pointer(repository, "production", digest)
        self.assertEqual(opener.open.call_count, 3)
        registry.write_runtime_pointer(prepared, digest)
        push_token = opener.open.call_args_list[2].args[0]
        self.assertIn("%3Apull%2Cpush", push_token.full_url)
        request = opener.open.call_args.args[0]
        self.assertEqual(request.get_method(), "PUT")
        self.assertEqual(request.data, raw)
        self.assertEqual(request.get_header("Authorization"), "Bearer push-fixture")
        self.assertEqual(request.get_header("Content-type"), manifest["mediaType"])
        self.assertTrue(request.full_url.endswith("/manifests/production-current"))

    def test_runtime_pointer_write_refuses_other_tags_and_requires_a_credential(self):
        opener = Mock()
        repository = "ghcr.io/chronoaiproject/cma-runtime"
        digest = "sha256:" + "a" * 64
        for environment, reference in (("staging", digest), ("production", "latest"), ("other", digest)):
            with self.subTest(environment=environment, reference=reference), self.assertRaises(Refusal):
                Registry(opener).prepare_runtime_pointer(repository, environment, reference)
        opener.open.assert_not_called()

    def test_settle_read_waits_for_a_just_written_tag_and_names_a_lasting_absence(self):
        manifest = {"schemaVersion": 2, "mediaType": "application/vnd.oci.image.manifest.v1+json",
                    "config": {"digest": "sha256:" + "a" * 64}, "layers": []}
        raw = json.dumps(manifest).encode()
        digest = "sha256:" + hashlib.sha256(raw).hexdigest()
        missing = urllib.error.HTTPError("https://ghcr.io/fixture", 404, "private", {}, None)
        waits = []
        opener = Mock()
        opener.open.side_effect = [self.response({"token": "fixture"}), missing, missing,
                                   self.response(raw, {"Docker-Content-Digest": digest})]
        registry = Registry(opener, retry={"sleep": waits.append})
        self.assertEqual(registry.manifest("ghcr.io/chronoaiproject/cma-agent-manager", "0.21.0", settle=True),
                         (digest, manifest))
        self.assertEqual(len(waits), 2)
        # A lasting 404 ends with the named refusal after the settle attempts.
        opener = Mock()
        opener.open.side_effect = [self.response({"token": "fixture"})] + [missing] * 6
        registry = Registry(opener, retry=NO_WAIT)
        with self.assertRaises(Refusal) as caught:
            registry.manifest("ghcr.io/chronoaiproject/cma-agent-manager", "0.21.0", settle=True)
        self.assertEqual(str(caught.exception), "Registry write is not visible yet")
        self.assertEqual(opener.open.call_count, 7)
        # Without settle, a 404 for an expected tag still fails at once; other statuses keep their meaning.
        for status, calls in ((404, 2), (401, 2), (500, 4)):
            error = urllib.error.HTTPError("https://ghcr.io/fixture", status, "private", {}, None)
            opener = Mock()
            opener.open.side_effect = [self.response({"token": "fixture"})] + [error] * 6
            with self.subTest(status=status), self.assertRaises(Refusal) as caught:
                Registry(opener, retry=NO_WAIT).manifest("ghcr.io/chronoaiproject/cma-web", "0.10.1")
            self.assertEqual(str(caught.exception), "Registry request failed")
            self.assertEqual(opener.open.call_count, calls)

    def test_transport_failures_repeat_and_then_read_the_same_manifest(self):
        manifest = {"schemaVersion": 2, "mediaType": "application/vnd.oci.image.manifest.v1+json",
                    "config": {"digest": "sha256:" + "a" * 64}, "layers": []}
        raw = json.dumps(manifest).encode()
        digest = "sha256:" + hashlib.sha256(raw).hexdigest()
        truncated = Mock(headers={})
        truncated.read.side_effect = http.client.IncompleteRead(b"{")
        for failure in (urllib.error.URLError(ConnectionResetError()), TimeoutError(),
                        contextlib.nullcontext(truncated)):
            with self.subTest(failure=type(failure).__name__):
                opener, sleep = Mock(), Mock()
                opener.open.side_effect = [self.response({"token": "fixture"}), failure,
                                           self.response(raw, {"Docker-Content-Digest": digest})]
                registry = Registry(opener, retry={"sleep": sleep})
                self.assertEqual(registry.manifest("ghcr.io/chronoaiproject/cma-web", digest), (digest, manifest))
                self.assertEqual(opener.open.call_count, 3)
                sleep.assert_called_once()
        # A digest mismatch is content, not transport: it is read once and refused.
        opener = Mock()
        opener.open.side_effect = [self.response({"token": "fixture"}),
                                   self.response(raw, {"Docker-Content-Digest": "sha256:" + "b" * 64})]
        with self.assertRaisesRegex(Refusal, "different manifest digest"):
            Registry(opener, retry=NO_WAIT).manifest("ghcr.io/chronoaiproject/cma-web", "0.10.1")
        self.assertEqual(opener.open.call_count, 2)

    def test_read_budget_shortens_each_read_and_refuses_once_spent(self):
        now = [0.0]
        opener = Mock()
        opener.open.side_effect = lambda _request, timeout: self.response({"token": "fixture"})
        registry = Registry(opener, retry={"clock": lambda: now[0]}, deadline=50.0)
        now[0] = 35.0
        self.assertEqual(registry.token("ghcr.io/chronoaiproject/cma-web"), "fixture")
        self.assertEqual(opener.open.call_args.kwargs["timeout"], 15.0)
        now[0] = 50.0
        with self.assertRaisesRegex(Refusal, "budget expired"):
            registry.token("ghcr.io/chronoaiproject/cma-web")
        self.assertEqual(opener.open.call_count, 1)

    def test_digest_mismatch_large_response_and_registry_redirect_fail_closed(self):
        opener = Mock()
        opener.open.side_effect = [self.response({"token": "fixture"}), self.response(b"{}", {"Docker-Content-Digest": "sha256:" + "a" * 64})]
        with self.assertRaises(Refusal):
            Registry(opener).manifest("ghcr.io/chronoaiproject/cma-web", "0.10.1")
        opener.open.side_effect = [self.response(b"x" * (16 * 1024 * 1024 + 1))]
        with self.assertRaises(Refusal):
            Registry(opener).token("ghcr.io/chronoaiproject/cma-web")
        self.assertIsNone(NoRedirect().redirect_request(None, None, 302, "", {}, "https://example.invalid"))

    def test_first_package_publication_authenticates_token_exchange_without_argv(self):
        opener = Mock()
        opener.open.return_value = self.response({"token": "fixture-registry"})
        registry = Registry(opener, credential="fixture", actor="actor")
        self.assertEqual(registry.token("ghcr.io/chronoaiproject/cma-web"), "fixture-registry")
        request = opener.open.call_args.args[0]
        self.assertTrue(request.get_header("Authorization").startswith("Basic "))
        self.assertNotIn("fixture", request.full_url)
        self.assertIn("%3Apull", request.full_url)
        with self.assertRaises(Refusal):
            Registry(credential="fixture")


if __name__ == "__main__":
    unittest.main()
