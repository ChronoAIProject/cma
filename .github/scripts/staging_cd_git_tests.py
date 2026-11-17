"""Exercise actual git reservation/finalization against a disposable bare remote."""
import json
import os
import subprocess
import tempfile
import unittest
from pathlib import Path
from unittest.mock import Mock

from staging_cd_common import Refusal
from staging_cd_github import GitHub
from staging_cd_receipts import published_receipt
from staging_cd_reservations import Reservations, next_reservation


class GitPublicationTests(unittest.TestCase):
    def setUp(self):
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
        self.publisher = Reservations(self.api, "fixture", self.work, self.runner)

    def git(self, *args, data=None, remote=False):
        command = ["git", "--git-dir", str(self.remote), *args] if remote else ["git", *args]
        return subprocess.run(command, cwd=self.work, env=self.env, input=data, text=True,
                              capture_output=True, check=True, timeout=10).stdout.strip()

    def runner(self, args, **kwargs):
        args = [str(self.remote) if value == "https://github.com/ChronoAIProject/cma.git" else value for value in args]
        return subprocess.run(args, **kwargs)

    def request(self, path):
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


if __name__ == "__main__":
    unittest.main()
