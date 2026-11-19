"""Captured GitHub API calls and immutable component tag publication."""
import json
import os
import subprocess

from staging_cd_common import Refusal, matches, require
from staging_cd_history import DevelopHistory
from staging_cd_watermark import require_current_versions
from staging_cd_receipts import parse_annotation
from staging_cd_versions import DEPLOYABLES, REPOSITORY, version_tuple


class GitHubNotFound(Refusal):
    """An explicit HTTP 404, distinct from authentication or transport failure."""


class GitHub:
    def require_current_versions(self, receipts):
        require_current_versions(receipts, runner=self.runner)

    def __init__(self, token, runner=subprocess.run, *, local_tags=None):
        require(isinstance(token, str) and token and not any(c.isspace() for c in token),
                "GitHub credential is missing or malformed")
        self.env = {k: v for k, v in os.environ.items() if k in ("PATH", "HOME", "LANG", "LC_ALL")}
        self.env.update(GH_TOKEN=token, GH_HOST="github.com", GH_PROMPT_DISABLED="1")
        self.runner = runner
        self.local_tags = local_tags
        self._history = None

    def history_at(self, head, *, root):
        if self._history is None or self._history.head != head:
            self._history = DevelopHistory.from_head(head, root=root, runner=self.runner)
        return self._history

    def history(self):
        if self._history is None:
            self._history = DevelopHistory.load(self.request)
        return self._history

    def history_containing(self, source):
        history = self.history()
        if source not in history.positions and history is self._history:
            # A long-lived publisher can observe a tag created after its first
            # history read. Re-read develop before judging the new tag's source.
            self._history = DevelopHistory.load(self.request, runner=self.runner)
            history = self._history
        history.position(source)
        return history

    def request(self, path, payload=None, *, allow_not_found=False, runner=None):
        require(isinstance(path, str) and not path.startswith(("/", "http"))
                and ".." not in path and "\n" not in path, "GitHub API path is invalid")
        if self.local_tags is not None and payload is None and not allow_not_found:
            tag = self.local_tags(path)
            if tag is not None:
                return tag
        args = ["gh", "api", "--hostname", "github.com", "-H", "Accept: application/vnd.github+json",
                "-H", "X-GitHub-Api-Version: 2022-11-28", "repos/" + REPOSITORY + "/" + path]
        if allow_not_found:
            require(payload is None, "Optional GitHub reads cannot write")
            args += ["--include", "--method", "GET"]
        if payload is not None:
            args += ["--method", "POST", "--input", "-"]
        try:
            result = (runner or self.runner)(args, input=None if payload is None else json.dumps(payload),
                                 env=self.env, text=True, capture_output=True, timeout=60, check=False)
        except (OSError, subprocess.SubprocessError) as error:
            raise Refusal("GitHub API request failed") from error
        data = result.stdout
        require(len(data) <= 16 * 1024 * 1024, "GitHub API request failed")
        if allow_not_found:
            headers, separator, data = data.replace("\r\n", "\n").partition("\n\n")
            status = headers.split("\n", 1)[0]
            require(separator and matches(r"HTTP/[0-9.]+ [1-5][0-9]{2}(?: [^\r\n]*)?", status),
                    "GitHub API response is invalid")
            code = int(status.split(" ", 2)[1])
            if code == 404 and result.returncode == 1:
                try:
                    error = json.loads(data)
                except (TypeError, ValueError) as error:
                    raise Refusal("GitHub API response is invalid") from error
                require(isinstance(error, dict) and error.get("status") == "404",
                        "GitHub API response is invalid")
                raise GitHubNotFound("GitHub resource was not found")
            require(code == 200, "GitHub API request failed")
        require(result.returncode == 0 and len(result.stdout) <= 16 * 1024 * 1024,
                "GitHub API request failed")
        try:
            value = json.loads(data)
        except (TypeError, ValueError) as error:
            raise Refusal("GitHub API response is invalid") from error

        if payload is None and matches(r"git/tags/[0-9a-f]{40}", path):
            require(isinstance(value, dict) and value.get("sha") == path.split("/")[-1],
                    "GitHub tag object differs from advertised ID")
        return value

    def tags(self, component=None):
        require(component is None or component in DEPLOYABLES, "Unknown component tag inventory")
        result = []
        for key, name in DEPLOYABLES.items():
            if component is not None and key != component:
                continue
            rows = self.request("git/matching-refs/tags/" + name + "/v")
            require(isinstance(rows, list), "Component tag inventory is invalid")
            for row in rows:
                require(isinstance(row, dict) and isinstance(row.get("ref"), str)
                        and row["ref"].startswith("refs/tags/" + name + "/v")
                        and isinstance(row.get("object"), dict) and row["object"].get("type") == "tag"
                        and matches(r"[0-9a-f]{40}", row["object"].get("sha")),
                        "Component version must be an annotated tag")
                tag_name = row["ref"][len("refs/tags/"):]
                version_tuple(tag_name[len(name + "/v"):])
                tag = self.request("git/tags/" + row["object"]["sha"])
                require(isinstance(tag, dict) and tag.get("tag") == tag_name
                        and isinstance(tag.get("object"), dict) and tag["object"].get("type") == "commit"
                        and matches(r"[0-9a-f]{40}", tag["object"].get("sha")),
                        "Component version tag identity differs")
                receipt = parse_annotation(tag.get("message"))
                require(receipt["tag"] == tag_name and receipt["source"] == tag["object"]["sha"],
                        "Tag receipt differs from the annotated commit")
                if key == "am":
                    self.history_containing(receipt["source"])
                else:
                    self.history().position(receipt["source"])
                result.append({"oid": row["object"]["sha"], "name": tag_name, "annotation_name": tag_name, "type": "tag",
                               "target_type": "commit", "target": tag["object"]["sha"], "receipt": receipt})
        require(len({tag["name"] for tag in result}) == len(result), "Duplicate component version tag")
        return result

