"""Exercise actual git reservation/finalization against a disposable bare remote."""
import json
import os
import subprocess
import tempfile
import unittest
from pathlib import Path
from unittest.mock import Mock, patch

from functools import partial
from staging_cd_allocate import allocate_queue as allocate_implementation
from staging_cd_retirements import Retirements
from staging_cd_common import Refusal
from staging_cd_finalize import finalize
from staging_cd_github import GitHub
from staging_cd_history import DevelopHistory
from staging_cd_receipts import published_receipt
from staging_cd_reservations import Reservations, next_reservation
from staging_cd_resolve import resolve, verify


allocate_queue = partial(allocate_implementation, retirements=Retirements({"decisions": []}, []))


class GitPublicationTests(unittest.TestCase):
    def setUp(self):
        guard = patch("staging_cd_finalize.guard_retired")
        guard.start()
        self.addCleanup(guard.stop)
        self.directory = tempfile.TemporaryDirectory()
        self.addCleanup(self.directory.cleanup)
        self.root = Path(self.directory.name)
        self.remote, self.work = self.root / "remote.git", self.root / "work"
        self.work.mkdir()
        self.env = dict(os.environ, GIT_CONFIG_NOSYSTEM="1", GIT_CONFIG_GLOBAL=os.devnull,
                        GIT_AUTHOR_NAME="Fixture", GIT_AUTHOR_EMAIL="fixture@example.invalid",
                        GIT_COMMITTER_NAME="Fixture", GIT_COMMITTER_EMAIL="fixture@example.invalid")
        self.git("init", "--quiet", "--bare", str(self.remote))
        self.git("init", "--quiet", str(self.work))
        self.tree = self.git("mktree", data="")
        self.source = self.git("commit-tree", self.tree, data="Synthetic release\n")
        self.git("push", str(self.remote), self.source + ":refs/heads/develop")
        self.api = GitHub("fixture")
        self.api.request = self.request
        self.api.history = lambda: DevelopHistory.load(self.request, root=self.work, runner=self.runner)
        self.publisher = Reservations(self.api, "fixture", self.work, self.runner)

    def git(self, *args, data=None, remote=False):
        command = ["git", "--git-dir", str(self.remote), *args] if remote else ["git", *args]
        return subprocess.run(command, cwd=self.work, env=self.env, input=data, text=True,
                              capture_output=True, check=True, timeout=10).stdout.strip()

    def runner(self, args, **kwargs):
        args = [str(self.remote) if value == "https://github.com/ChronoAIProject/cma.git" else value for value in args]
        return subprocess.run(args, **kwargs)

    def request(self, path):
        if path == "git/ref/heads/develop":
            return {"ref": "refs/heads/develop", "object": {"type": "commit", "sha":
                    self.git("rev-parse", "refs/heads/develop", remote=True)}}
        if path.startswith("git/matching-refs/"):
            prefix = "refs/" + path[len("git/matching-refs/"):]
            lines = self.git("for-each-ref", "--format=%(refname) %(objectname) %(objecttype)", remote=True).splitlines()
            return [{"ref": row[0], "object": {"sha": row[1], "type": row[2]}}
                    for line in lines if (row := line.split()) and row[0].startswith(prefix)]
        if path.startswith("git/tags/"):
            body = self.git("cat-file", "-p", path[len("git/tags/"):], remote=True)
            header, message = body.split("\n\n", 1)
            fields = dict(line.split(" ", 1) for line in header.splitlines())
            return {"tag": fields["tag"], "message": message, "object": {"type": fields["type"], "sha": fields["object"]}}
        raise AssertionError("Unexpected fixture endpoint")

    def reservation(self, run_id=1):
        return next_reservation("web", self.source, self.tree, 42, "patch", run_id, 1,
                                self.api.tags(), self.publisher.list())

    def receipt(self, reservation):
        identity = dict(reservation, schema="cma-image-release-v1", build="c" * 64)
        return published_receipt(identity, "sha256:" + "d" * 64, "e" * 64,
                                 {"detected": 1, "blocking": 0, "advisory": 1, "reviewed": 0, "allowlisted": 0})

    def test_new_ref_is_required_and_a_different_owner_loses_the_collision(self):
        value = self.reservation()
        self.assertTrue(self.publisher.insert(value))
        self.assertEqual(self.publisher.list(), [value])
        with self.assertRaisesRegex(Refusal, "up-to-date"):
            self.publisher.insert(value)
        self.assertFalse(self.publisher.insert(dict(value, run_id=2)))
        self.assertEqual(self.reservation(2)["version"], "0.10.2")

    def test_verified_final_holds_digest_before_successful_pending_deletion(self):
        value = self.reservation()
        self.publisher.insert(value)
        receipt = self.receipt(value)
        with self.assertRaises(Refusal):
            self.publisher.finalize(receipt, Mock(return_value=False))
        self.assertEqual(self.publisher.list(), [value])
        self.assertEqual(self.api.tags(), [])
        self.assertTrue(self.publisher.finalize(receipt, Mock(return_value=True)))
        self.assertEqual(self.api.tags("web")[0]["receipt"], receipt)
        self.assertEqual(self.publisher.list(), [])
        self.assertFalse(self.publisher.finalize(receipt, Mock(return_value=True)))
        self.assertEqual(len(self.api.tags("web")), 1)

    def test_ambiguous_push_is_reconciled_by_exact_owner_without_duplicate_version(self):
        real = self.publisher.runner
        def ambiguous(args, **kwargs):
            result = real(args, **kwargs)
            if args[1] == "push":
                result.returncode = 1
                result.stdout = ""
            return result
        self.publisher.runner = ambiguous
        value = self.reservation()
        self.assertTrue(self.publisher.insert(value))
        self.assertTrue(self.publisher.finalize(self.receipt(value), Mock(return_value=True)))

    def test_failed_finalization_keeps_burned_pending_and_cannot_finalize_twice_for_same_source(self):
        first = self.reservation()
        self.publisher.insert(first)
        second = self.reservation(2)
        self.publisher.insert(second)
        self.publisher.finalize(self.receipt(second), Mock(return_value=True))
        with self.assertRaises(Refusal):
            self.publisher.finalize(self.receipt(first), Mock(return_value=True))
        self.assertEqual(self.publisher.list(), [first])
        self.assertEqual(self.api.tags()[0]["receipt"]["identity"]["version"], "0.10.2")

    def test_github_cli_keeps_credentials_out_of_argv_and_suppresses_errors(self):
        runner = Mock(return_value=Mock(returncode=0, stdout='{"ok":true}'))
        api = GitHub("fixture", runner)
        self.assertEqual(api.request("git/tags", {"tag": "synthetic"}), {"ok": True})
        call = runner.call_args
        self.assertNotIn("fixture", call.args[0])
        self.assertEqual(call.kwargs["env"]["GH_TOKEN"], "fixture")
        self.assertEqual(json.loads(call.kwargs["input"]), {"tag": "synthetic"})
        runner.return_value = Mock(returncode=1, stdout="private", stderr="private")
        with self.assertRaisesRegex(Refusal, "GitHub API request failed"):
            api.request("git/tags")
        runner.return_value = Mock(returncode=0, stdout="not JSON")
        with self.assertRaises(Refusal):
            api.request("git/tags")
        for path in ("https://example.invalid", "/git/tags", "../other", "git\ntags"):
            with self.assertRaises(Refusal):
                api.request(path)

    def test_older_failed_run_can_publish_burned_retry_but_never_replace_newer_source(self):
        older = self.source
        newer = self.git("commit-tree", self.tree, "-p", older, data="Later release\n")
        self.git("push", str(self.remote), newer + ":refs/heads/develop")
        active = True
        def request(path):
            if path.startswith("actions/runs/"):
                return {"id": 9, "run_attempt": 1, "event": "push", "head_branch": "develop",
                        "repository": {"full_name": "ChronoAIProject/cma"},
                        "status": "in_progress" if active else "completed"}
            if path.startswith(("commits/", "pulls/")):
                is_older = older in path or path == "pulls/1"
                source, number, level = (older, 1, "minor") if is_older else (newer, 2, "patch")
                pr = {"number": number, "merged": True, "merged_at": "fixture", "merge_commit_sha": source,
                      "base": {"ref": "develop", "repo": {"full_name": "ChronoAIProject/cma"}},
                      "labels": [{"name": "version-upgrade:" + level}]}
                return [pr] if path.startswith("commits/") else pr
            return self.request(path)
        self.api.request = request
        queue = {"head": newer, "event_source": newer, "changes": [
            {"source": source, "tree": self.tree, "deployables": ["cma-web"]} for source in (older, newer)]}
        pending = next_reservation("web", older, self.tree, 1, "minor", 9, 1, [], [])
        self.publisher.insert(pending)
        registry = Mock()
        registry.exists.return_value = True

        def publish(run_id, digest):
            matrix = allocate_queue(queue, self.api, self.publisher, run_id, 1)
            tasks = json.loads(matrix["include"][0]["tasks"])
            self.assertEqual(len(tasks), 1)
            receipt = dict(self.receipt(tasks[0]["reservation"]), digest=digest)
            self.assertEqual(finalize(tasks, [receipt], self.publisher, registry), receipt)
            return receipt

        later = publish(10, "sha256:" + "d" * 64)
        self.assertEqual(later["source"], newer)
        self.assertEqual(later["identity"]["version"], "0.11.1")
        active = False
        queue["event_source"] = older
        retry = publish(11, "sha256:" + "a" * 64)
        self.assertEqual(retry["source"], older)
        self.assertEqual(retry["identity"]["version"], "0.12.0")
        self.assertEqual(self.publisher.list(), [pending])  # Failed version stays burned.
        tags, history = self.api.tags("web"), self.api.history()
        registry.manifest.side_effect = lambda _repo, version: (
            next(receipt["digest"] for receipt in (later, retry) if receipt["identity"]["version"] == version), {})
        self.assertEqual(resolve("web", "latest", tags, registry, history), later)
        with self.assertRaisesRegex(Refusal, "newer published source"):
            verify(retry, tags, registry, history, automatic=True)
        self.assertEqual(verify(later, tags, registry, history, automatic=True), later)
        self.assertEqual(verify(retry, tags, registry, history, automatic=False), retry)
        self.assertEqual(allocate_queue(queue, self.api, self.publisher, 12, 1), {"include": []})
        queue["event_source"] = newer
        resumed = allocate_queue(queue, self.api, self.publisher, 13, 1)
        self.assertEqual(json.loads(resumed["include"][0]["tasks"]),
                         [{"component": "web", "mode": "resume", "receipt": later}])

    def test_tag_enumeration_and_resolution_reject_unrelated_missing_and_side_branch_sources(self):
        unrelated = self.git("commit-tree", self.tree, data="Unrelated root\n")
        side = self.git("commit-tree", self.tree, "-p", self.source, data="Side branch\n")
        merge = self.git("commit-tree", self.tree, "-p", self.source, "-p", side, data="Merged branch\n")
        self.git("push", str(self.remote), merge + ":refs/heads/develop")
        history = self.api.history()
        self.assertGreater(history.position(merge), history.position(self.source))
        registry = Mock()
        registry.exists.return_value = True
        registry.manifest.return_value = ("sha256:" + "d" * 64, {})
        for source in (self.source, unrelated, "f" * 40, side):
            receipt = self.receipt(next_reservation("web", source, self.tree, 42, "patch", 1, 1, [], []))
            def request(path):
                if path.startswith("git/matching-refs/"):
                    return [{"ref": "refs/tags/" + receipt["tag"], "object": {"type": "tag", "sha": "b" * 40}}]
                if path.startswith("git/tags/"):
                    from staging_cd_receipts import annotation
                    return {"tag": receipt["tag"], "object": {"type": "commit", "sha": source},
                            "message": annotation(receipt)}
                return self.request(path)
            api = GitHub("fixture")
            api.request = request
            api.history = lambda: history
            from staging_cd_receipts_tests import fixture_tag
            with self.subTest(source=source):
                if source == self.source:
                    self.assertEqual(resolve("web", "latest", api.tags("web"), registry, history), receipt)
                else:
                    with self.assertRaises(Refusal):
                        api.tags("web")
                    with self.assertRaises(Refusal):
                        resolve("web", "latest", [fixture_tag(receipt)], registry, history)

    def test_unavailable_or_incomplete_ancestry_fails_closed_and_git_has_no_credentials(self):
        observed = []
        def runner(args, **kwargs):
            observed.append(kwargs["env"])
            return self.runner(args, **kwargs)
        history = DevelopHistory.load(self.request, root=self.work, runner=runner)
        self.assertEqual(history.position(self.source), 0)
        self.assertTrue(observed)
        for env in observed:
            self.assertTrue(set(env).issubset({"PATH", "LANG", "LC_ALL", "GIT_CONFIG_NOSYSTEM",
                                             "GIT_CONFIG_GLOBAL", "GIT_TERMINAL_PROMPT", "GIT_NO_REPLACE_OBJECTS"}))
        failure = Mock(return_value=Mock(returncode=1, stdout=""))
        with self.assertRaises(Refusal):
            DevelopHistory.load(self.request, root=self.work, runner=failure)
        incomplete = Mock(side_effect=[Mock(returncode=0, stdout="true"), Mock(returncode=0, stdout=""),
                                       Mock(returncode=0, stdout="true")])
        with self.assertRaises(Refusal):
            DevelopHistory.load(self.request, root=self.work, runner=incomplete)
        self.assertIn("--unshallow", incomplete.call_args_list[1].args[0])

    def test_default_shallow_runner_checkout_is_completed_before_source_ordering(self):
        newer = self.git("commit-tree", self.tree, "-p", self.source, data="Later release\n")
        self.git("push", str(self.remote), newer + ":refs/heads/develop")
        shallow = self.root / "shallow"
        self.git("clone", "--quiet", "--depth=1", "--branch=develop", self.remote.as_uri(), str(shallow))
        self.assertEqual(self.git("-C", str(shallow), "rev-parse", "--is-shallow-repository"), "true")
        history = DevelopHistory.load(self.request, root=shallow, runner=self.runner)
        self.assertEqual(history.head, newer)
        self.assertGreater(history.position(newer), history.position(self.source))
        self.assertEqual(self.git("-C", str(shallow), "rev-parse", "--is-shallow-repository"), "false")


if __name__ == "__main__":
    unittest.main()
