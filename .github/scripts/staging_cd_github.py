"""Captured GitHub API calls and immutable component tag publication."""
import copy
import json
import os
import subprocess
import threading
import time

import ci_retry
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

    def __init__(self, token, runner=subprocess.run, *, local_tags=None, retry=None, deadline=None):
        require(isinstance(token, str) and token and not any(c.isspace() for c in token),
                "GitHub credential is missing or malformed")
        self.env = {k: v for k, v in os.environ.items() if k in ("PATH", "HOME", "LANG", "LC_ALL")}
        self.env.update(GH_TOKEN=token, GH_HOST="github.com", GH_PROMPT_DISABLED="1")
        self.runner = runner
        self.local_tags = local_tags
        # Keyword arguments for ci_retry.call: a caller's deadline, clock and sleep.
        self.retry = dict(retry or {})
        # An absolute budget on the retry clock for every read. It shortens each
        # attempt and ends repeats, so a step refuses before its job is killed.
        self.deadline = deadline
        self._history = None
        # Allocation reads tags on several threads, and each may need develop
        # history first. One lock keeps them to a single fetch into the checkout:
        # concurrent fetches into one repository race on its lock files.
        self._history_lock = threading.RLock()
        # Annotated tag objects are immutable and keyed by their verified ID.
        # Live ref inventories are never cached; each read is still a fence.
        self._tags = {}

    def history_at(self, head, *, root):
        with self._history_lock:
            if self._history is None or self._history.head != head:
                self._history = DevelopHistory.from_head(head, root=root, runner=self.runner)
            return self._history

    def history(self):
        with self._history_lock:
            if self._history is None:
                self._history = DevelopHistory.load(self.request, runner=self.runner)
            return self._history

    def history_containing(self, source):
        with self._history_lock:
            history = self.history()
            if source not in history.positions and history is self._history:
                # A long-lived publisher can observe a tag created after its first
                # history read. Re-read develop before judging the new tag's source.
                self._history = DevelopHistory.load(self.request, runner=self.runner)
                history = self._history
        history.position(source)
        return history

    def request(self, path, payload=None, *, allow_not_found=False, runner=None, method=None, attempts=3,
                deadline=None):
        require(isinstance(path, str) and not path.startswith(("/", "http"))
                and ".." not in path and "\n" not in path, "GitHub API path is invalid")
        require(method is None or method == "PATCH" and payload is not None, "GitHub mutation method is invalid")
        oid = path[len("git/tags/"):] if payload is None and not allow_not_found \
            and matches(r"git/tags/[0-9a-f]{40}", path) else None
        if oid in self._tags:
            return copy.deepcopy(self._tags[oid])
        if self.local_tags is not None and payload is None and not allow_not_found:
            tag = self.local_tags(path)
            if tag is not None:
                return self.remember(oid, tag)
        require(payload is None or not allow_not_found, "Optional GitHub reads cannot write")
        # Reads ask for the status line and headers, the only explicit throttling signal.
        args = ["gh", "api", "--hostname", "github.com", "-H", "Accept: application/vnd.github+json",
                "-H", "X-GitHub-Api-Version: 2022-11-28", *(["--include", "--method", "GET"] if payload is None else []),
                "repos/" + REPOSITORY + "/" + path]
        if payload is not None:
            args += ["--method", method or "POST", "--input", "-"]

        def once(timeout=60):
            try:
                result = (runner or self.runner)(args, input=None if payload is None else json.dumps(payload),
                                                 env=self.env, text=True, capture_output=True, timeout=timeout,
                                                 check=False)
            except (OSError, subprocess.SubprocessError) as error:
                raise Refusal("GitHub API request failed") from error
            return self.response(path, payload, result, allow_not_found)

        if payload is not None:
            # A write runs once. Its caller settles an uncertain outcome by reading back.
            return once()
        budget = min((value for value in (self.deadline, deadline) if value is not None), default=None)
        policy = ci_retry.bounded(self.retry, budget)

        def attempt():
            timeout = ci_retry.attempt_timeout(60, budget, policy.get("clock", time.monotonic))
            if timeout is None:
                # A slow network spent the budget; GitHub refused nothing.
                raise Refusal("GitHub read budget expired") from ci_retry.TransientError()
            return once(timeout)
        value = ci_retry.call(attempt, name="github-read", classify=ci_retry.classify, attempts=attempts, **policy)
        return value if oid is None else self.remember(oid, value)

    def remember(self, oid, value):
        if oid is not None:
            self._tags[oid] = copy.deepcopy(value)
        return value

    def preload_tags(self, rows):
        """Read the absent objects of one validated live ref inventory from the checkout at once."""
        if self.local_tags is None:
            return
        wanted = {row.get("ref"): row["object"]["sha"] for row in rows if row["object"]["sha"] not in self._tags}
        if wanted:
            self._tags.update(self.local_tags.load(wanted))

    @staticmethod
    def response(path, payload, result, allow_not_found):
        data = result.stdout
        require(len(data) <= 16 * 1024 * 1024, "GitHub API request failed")
        # Only gh's diagnostic can tell a transport failure from a refusal.
        diagnostic = getattr(result, "stderr", None)
        code = headers = None
        if payload is None:
            head, separator, body = data.replace("\r\n", "\n").partition("\n\n")
            status = head.split("\n", 1)[0]
            if separator and matches(r"HTTP/[0-9.]+ [1-5][0-9]{2}(?: [^\r\n]*)?", status):
                code, headers, data = int(status.split(" ", 2)[1]), head, body
            elif result.returncode != 0:
                # No HTTP response arrived at all.
                raise Refusal("GitHub API request failed") from ci_retry.cli_cause(diagnostic)
            else:
                # gh prints a status line for every response; only a missing 404 needs one.
                require(not allow_not_found, "GitHub API response is invalid")
        if code is not None:
            if code == 404 and allow_not_found and result.returncode == 1:
                try:
                    error = json.loads(data)
                except (TypeError, ValueError) as error:
                    raise Refusal("GitHub API response is invalid") from error
                require(isinstance(error, dict) and error.get("status") == "404",
                        "GitHub API response is invalid")
                raise GitHubNotFound("GitHub resource was not found")
            if code != 200:
                failure = Refusal("GitHub API request failed")
                failure.status = code  # Lets a caller tell a missing or forbidden object from an outage.
                raise failure from ci_retry.status_cause(code, headers)
        if result.returncode != 0:
            raise Refusal("GitHub API request failed") from ci_retry.cli_cause(diagnostic)
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
                version_tuple(row["ref"][len("refs/tags/" + name + "/v"):])
            self.preload_tags(rows)
            for row in rows:
                tag_name = row["ref"][len("refs/tags/"):]
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

