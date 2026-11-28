"""Public GHCR manifest reads fail closed on authentication and transport errors."""
import hashlib
import base64
import http.client
import json
import time
import urllib.error
import urllib.parse
import urllib.request

import ci_retry
from staging_cd_common import Refusal, matches, require
from staging_cd_targets import RUNTIME_TARGETS
from staging_cd_versions import DEPLOYABLES, SEMVER

MEDIA = "application/vnd.oci.image.manifest.v1+json, application/vnd.docker.distribution.manifest.v2+json"
WEB_AUXILIARY = r"asset-sha256-[0-9a-f]{64}"
WEB_REPOSITORY = "ghcr.io/chronoaiproject/cma-web"
JOURNEY_REPOSITORY = "ghcr.io/chronoaiproject/cma-journey-probe"
JOURNEY_TAG = r"sha256-[0-9a-f]{64}"
REPOSITORIES = frozenset("ghcr.io/chronoaiproject/" + name for name in DEPLOYABLES.values()) | {JOURNEY_REPOSITORY}
RUNTIME_REPOSITORIES = frozenset("ghcr.io/chronoaiproject/" + DEPLOYABLES[key] for key in RUNTIME_TARGETS)
STAGING_DESKTOP_TAG_PREFIX = "staging-desktop-sha256-"
RUNTIME_POINTER_TAGS = {"staging": "staging-current", "production": "production-current"}


class NoRedirect(urllib.request.HTTPRedirectHandler):
    def redirect_request(self, req, fp, code, msg, headers, newurl):
        return None


class Registry:
    def __init__(self, opener=None, *, credential=None, actor=None, retry=None, deadline=None):
        self.opener = opener or urllib.request.build_opener(NoRedirect())
        require(credential is None or (isinstance(credential, str) and bool(credential)
                                      and isinstance(actor, str) and bool(actor)),
                "Registry credential is invalid")
        self.credential, self.actor = credential, actor
        # Keyword arguments for ci_retry.call; reads are idempotent GETs.
        self.retry = dict(retry or {})
        # An absolute budget on the retry clock: it shortens each read and ends repeats.
        self.deadline = deadline

    # A tag or digest written moments ago can be briefly unknown at another
    # registry edge. Verification after a write waits for it, inside the budget.
    SETTLE = {"attempts": 6, "base": 2.0, "cap": 15.0}

    def read(self, url, headers, *, absent=False, settle=False):
        policy = ci_retry.bounded({**self.SETTLE, **self.retry} if settle else self.retry, self.deadline)

        def once():
            timeout = ci_retry.attempt_timeout(30, self.deadline, policy.get("clock", time.monotonic))
            if timeout is None:
                raise Refusal("Registry read budget expired") from ci_retry.TransientError()
            try:
                with self.opener.open(urllib.request.Request(url, headers=headers), timeout=timeout) as response:
                    data = response.read(16 * 1024 * 1024 + 1)
                    require(len(data) <= 16 * 1024 * 1024, "Registry response exceeds limit")
                    return data, response.headers
            except urllib.error.HTTPError as error:
                if absent and error.code == 404:
                    return None
                if settle and error.code == 404:
                    raise Refusal("Registry write is not visible yet") from ci_retry.TransientError()
                raise Refusal("Registry request failed") from error
            except (OSError, urllib.error.URLError, http.client.HTTPException) as error:
                # A truncated body or dropped connection is a transport failure too.
                raise Refusal("Registry request failed") from error
        return ci_retry.call(once, name="registry-read", classify=ci_retry.classify, **policy)

    def token(self, repository):
        return self._token(repository, "pull")

    def _token(self, repository, actions):
        require(repository in REPOSITORIES, "Registry repository is not a canonical CMA image")
        scope = "repository:" + repository[len("ghcr.io/"):] + ":" + actions
        headers = {}
        if self.credential:
            headers["Authorization"] = "Basic " + base64.b64encode((self.actor + ":" + self.credential).encode()).decode()
        result = self.read("https://ghcr.io/token?" + urllib.parse.urlencode({"service": "ghcr.io", "scope": scope}), headers)
        require(result is not None, "Registry authentication is unavailable")
        payload = json.loads(result[0])
        token = payload.get("token")
        require(isinstance(token, str) and token and not any(c.isspace() for c in token),
                "Registry authentication is invalid")
        return token

    def manifest(self, repository, reference, *, absent=False, settle=False):
        auxiliary = repository == WEB_REPOSITORY and matches(WEB_AUXILIARY, reference)
        desktop = repository in RUNTIME_REPOSITORIES and matches(STAGING_DESKTOP_TAG_PREFIX + r"[0-9a-f]{64}", reference)
        tagged = (matches(JOURNEY_TAG, reference) if repository == JOURNEY_REPOSITORY
                  else auxiliary or desktop or matches(SEMVER, reference))
        require(tagged or matches(r"sha256:[0-9a-f]{64}", reference),
                "Registry reference is not immutable")
        return self._manifest(repository, reference, absent=absent, settle=settle)

    def runtime_pointer(self, repository, environment, *, settle=False):
        require(repository in RUNTIME_REPOSITORIES and environment in RUNTIME_POINTER_TAGS,
                "Runtime pointer repository or environment is invalid")
        result = self._manifest(repository, RUNTIME_POINTER_TAGS[environment], absent=not settle, settle=settle)
        return result[0] if result else None

    def prepare_runtime_pointer(self, repository, environment, digest):
        require(repository in RUNTIME_REPOSITORIES and environment in RUNTIME_POINTER_TAGS
                and matches(r"sha256:[0-9a-f]{64}", digest), "Runtime pointer write is invalid")
        require(self.credential is not None, "Runtime pointer writer needs a registry credential")
        manifest = self._manifest(repository, digest, absent=False, settle=False, raw=True)
        _, body, data = manifest
        token = self._token(repository, "pull,push")
        url = "https://ghcr.io/v2/" + repository[len("ghcr.io/"):] + "/manifests/" + RUNTIME_POINTER_TAGS[environment]
        return urllib.request.Request(url, data=data, method="PUT", headers={
            "Authorization": "Bearer " + token, "Content-Type": body["mediaType"],
        })

    def write_runtime_pointer(self, request, digest):
        timeout = ci_retry.attempt_timeout(30, self.deadline, self.retry.get("clock", time.monotonic))
        require(timeout is not None, "Registry write budget expired")
        with self.opener.open(request, timeout=timeout) as response:
            require(response.status == 201 and response.headers.get("Docker-Content-Digest", digest) == digest,
                    "Registry did not confirm the runtime pointer manifest")

    def _manifest(self, repository, reference, *, absent, settle, raw=False):
        token = self.token(repository)
        result = self.read("https://ghcr.io/v2/" + repository[len("ghcr.io/"):] + "/manifests/" + reference,
                           {"Authorization": "Bearer " + token, "Accept": MEDIA}, absent=absent, settle=settle)
        if result is None:
            return None
        data, headers = result
        digest = "sha256:" + hashlib.sha256(data).hexdigest()
        require(headers.get("Docker-Content-Digest") == digest
                and (not reference.startswith("sha256:") or reference == digest)
                and (not reference.startswith("sha256-") or reference == "sha256-" + digest[7:])
                and (not reference.startswith("asset-sha256-") or reference == "asset-sha256-" + digest[7:])
                and (not reference.startswith(STAGING_DESKTOP_TAG_PREFIX)
                     or reference == STAGING_DESKTOP_TAG_PREFIX + digest[7:]),
                "Registry returned a different manifest digest")
        body = json.loads(data)
        require(isinstance(body, dict) and body.get("schemaVersion") == 2
                and body.get("mediaType") in MEDIA.split(", ")
                and isinstance(body.get("config"), dict) and isinstance(body.get("layers"), list),
                "Staging registry manifest must be a single platform image")
        return (digest, body, data) if raw else (digest, body)

    def exists(self, repository, digest, *, settle=False):
        return self.manifest(repository, digest, settle=settle) is not None

