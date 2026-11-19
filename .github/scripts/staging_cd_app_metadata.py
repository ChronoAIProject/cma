"""Resolve and mask a private commit using a narrowly scoped App token."""
import json
import urllib.request

from staging_cd_common import matches, require

REPOSITORY = "ChronoAIProject/cma-infra"


class NoRedirect(urllib.request.HTTPRedirectHandler):
    def redirect_request(self, req, fp, code, msg, headers, newurl):
        return None


def github_reader(token):
    require(isinstance(token, str) and bool(token) and not any(c.isspace() for c in token),
            "Private checkout credential is malformed")
    opener = urllib.request.build_opener(NoRedirect)

    def read(path):
        # Callers supply only fixed installation endpoints or a validated SHA.
        request = urllib.request.Request("https://api.github.com/" + path, headers={
            "Authorization": "Bearer " + token, "Accept": "application/vnd.github+json",
            "X-GitHub-Api-Version": "2022-11-28", "User-Agent": "cma-deployment"})
        with opener.open(request, timeout=30) as response:
            require(response.status == 200, "Private repository metadata request failed")
            content = response.read(1024 * 1024 + 1)
        require(len(content) <= 1024 * 1024, "Private metadata response is too large")
        value = json.loads(content)
        require(isinstance(value, dict), "Private metadata response is malformed")
        return value

    return read


# API labels describe the response schema. Private identifiers and credentials
# can be short, so their protection must never depend on the metadata threshold.
MIN_METADATA_LENGTH = 8
PRIVATE_FIELD_PARTS = frozenset({
    "id", "sha", "login", "name", "email", "url", "ref", "branch", "message",
    "token", "tokens", "key", "keys", "secret", "secrets", "credential", "credentials",
    "password", "signature", "payload",
})
PUBLIC_ENUMS = {
    "visibility": {"private", "public", "internal"},
    "status": {"enabled", "disabled"},
    "user_view_type": {"private", "public"},
    "type": {"User", "Bot", "Organization", "commit", "tree", "blob", "tag"},
    "reason": {"valid", "invalid", "unsigned", "unknown_key", "not_signing_key", "expired_key",
               "gpgverify_error", "gpgverify_unavailable", "unknown_signature_type", "no_user",
               "unverified_email", "bad_email", "malformed_signature"},
}


def metadata_values(item, field="", protected=False):
    protected = protected or bool(PRIVATE_FIELD_PARTS.intersection(field.lower().split("_")))
    if isinstance(item, dict):
        for key, child in item.items():
            yield from metadata_values(child, key, protected)
    elif isinstance(item, list):
        for child in item:
            yield from metadata_values(child, field, protected)
    elif isinstance(item, str) and item.strip():
        if protected:
            yield item
        elif len(item.strip()) >= MIN_METADATA_LENGTH and item not in PUBLIC_ENUMS.get(field, ()):
            # Keep unfamiliar private metadata confidential without registering
            # incidental short strings as job-wide substring filters.
            yield item
    elif type(item) is int and (field == "id" or field.endswith("_id")):
        yield str(item)


def resolve_app_commit(token, revision, masker, reader_factory=github_reader):
    require(revision in ("", "main") or matches(r"[0-9a-f]{40}", revision),
            "Private checkout requires main or an explicit commit")
    masker(token)
    read = reader_factory(token)
    repositories = read("installation/repositories?per_page=100")
    masker(list(metadata_values(repositories)))
    entries = repositories.get("repositories")
    require(type(repositories.get("total_count")) is int and repositories["total_count"] == 1
            and isinstance(entries, list) and len(entries) == 1 and isinstance(entries[0], dict)
            and entries[0].get("full_name") == REPOSITORY and entries[0].get("private") is True,
            "Private checkout token has unexpected repository access")
    if revision in ("", "main"):
        reference = read(f"repos/{REPOSITORY}/git/ref/heads/main")
        require(reference.get("ref") == "refs/heads/main" and isinstance(reference.get("object"), dict)
                and reference["object"].get("type") == "commit"
                and matches(r"[0-9a-f]{40}", reference["object"].get("sha")),
                "Private main revision could not be resolved")
        revision = reference["object"]["sha"]
    commit = read(f"repos/{REPOSITORY}/git/commits/{revision}")
    # The selected revision is explicitly public provenance. Other metadata,
    # including parent/tree identities and commit text, remains confidential.
    masker(list(metadata_values({key: value for key, value in commit.items() if key != "sha"})))
    require(commit.get("sha") == revision and isinstance(commit.get("message"), str),
            "Private checkout commit differs")
    # Git emits the first paragraph as a folded subject during detached checkout.
    # Register private lines and the folded subject as units, including short
    # subjects. Splitting them into words would redact unrelated diagnostics.
    message = commit["message"]
    lines = message.splitlines()
    subject = " ".join(message.split("\n\n", 1)[0].splitlines())
    masker([subject, *lines])
    return revision
