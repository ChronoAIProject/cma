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
