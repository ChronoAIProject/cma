"""Bind release sources to a fresh, complete develop first-parent history."""
import os
import subprocess
from pathlib import Path

import ci_retry
from staging_cd_common import Refusal, matches, require


class DevelopHistory:
    def __init__(self, head, commits):
        require(bool(commits) and commits[0] == head and len(set(commits)) == len(commits)
                and all(matches(r"[0-9a-f]{40}", sha) for sha in commits),
                "Develop first-parent history is malformed")
        self.head = head
        self.positions = {sha: -index for index, sha in enumerate(commits)}

    def position(self, source):
        require(matches(r"[0-9a-f]{40}", source) and source in self.positions,
                "Release source is not on verified develop first-parent history")
        return self.positions[source]

    @classmethod
    def load(cls, request, *, root=None, runner=subprocess.run):
        reference = request("git/ref/heads/develop")
        require(isinstance(reference, dict) and reference.get("ref") == "refs/heads/develop"
                and isinstance(reference.get("object"), dict)
                and reference["object"].get("type") == "commit"
                and matches(r"[0-9a-f]{40}", reference["object"].get("sha")),
                "Develop head could not be verified")
        return cls.from_head(reference["object"]["sha"], root=root, runner=runner)

    @classmethod
    def from_head(cls, head, *, root=None, runner=subprocess.run, depth=None):
        require(matches(r"[0-9a-f]{40}", head), "Develop head could not be verified")
        # This repository is public. History reads need no publication, App or
        # deployment credential, and never update a branch or the worktree.
        env = {key: value for key, value in os.environ.items() if key in ("PATH", "LANG", "LC_ALL")}
        env.update(GIT_CONFIG_NOSYSTEM="1", GIT_CONFIG_GLOBAL=os.devnull,
                   GIT_TERMINAL_PROMPT="0", GIT_NO_REPLACE_OBJECTS="1")

        def git(*args):
            result = runner(["git", "-c", "credential.helper=", "-c", "http.extraHeader=", *args],
                            cwd=root or Path.cwd(), env=env, text=True, capture_output=True,
                            timeout=120, check=False)
            if result.returncode != 0:
                raise Refusal("Develop ancestry could not be verified") from ci_retry.cli_cause(result.stderr)
            require(len(result.stdout) <= 16 * 1024 * 1024, "Develop ancestry could not be verified")
            return result.stdout.strip()

        shallow = git("rev-parse", "--is-shallow-repository")
        require(shallow in ("true", "false"), "Develop ancestry completeness is unknown")
        if depth is None:
            git("fetch", "--no-tags", "--filter=blob:none", *(["--unshallow"] if shallow == "true" else []),
                "https://github.com/ChronoAIProject/cma.git", head)
            require(git("rev-parse", "--is-shallow-repository") == "false",
                    "Develop ancestry is incomplete")
        else:
            # Planning needs only a positive first-parent witness. Fetching a
            # bounded history for the advertised commit leaves the checkout pinned.
            depth_option = (f"--depth={depth}",) if shallow == "true" else ()
            git("fetch", "--no-tags", "--filter=blob:none", *depth_option, "--no-write-fetch-head",
                "https://github.com/ChronoAIProject/cma.git", head)
        # Membership in this chain proves ancestry and excludes a merged PR's
        # side-branch commits. SemVer and tag creation time do not order sources.
        limit = () if depth is None else (f"--max-count={depth}",)
        return cls(head, git("rev-list", "--first-parent", *limit, head).splitlines())
