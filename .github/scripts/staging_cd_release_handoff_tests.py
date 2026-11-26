"""Exercise the release publisher's history-only back-merge against a real remote."""

import os
from pathlib import Path
import subprocess
from tempfile import TemporaryDirectory
import unittest

import yaml


GITHUB = Path(__file__).resolve().parents[1]
SCRIPT = GITHUB / "prepare-release-back-merge.sh"


class ReleaseHandoffTests(unittest.TestCase):
    def setUp(self):
        directory = TemporaryDirectory()
        self.addCleanup(directory.cleanup)
        self.root = Path(directory.name)
        self.repo = self.root / "work"
        self.remote = self.root / "remote.git"
        self.summary = self.root / "summary.md"
        self.repo.mkdir()
        self.git("init", "--quiet", "--initial-branch=main")
        self.git("init", "--quiet", "--bare", str(self.remote))
        self.git("config", "user.name", "Fixture")
        self.git("config", "user.email", "fixture@example.com")
        self.commit("base", "base")
        self.git("checkout", "--quiet", "-b", "develop")
        self.commit("features", "on develop")
        self.git("checkout", "--quiet", "main")
        self.git("merge", "--quiet", "--no-ff", "-m", "Release", "develop")
        self.main = self.git("rev-parse", "HEAD")
        self.branch = f"chore/back-merge-main-{self.main}"
        self.ref = f"refs/heads/{self.branch}"
        self.git("remote", "add", "origin", str(self.remote))
        self.git("push", "--quiet", "origin", "main", "develop")

    def git(self, *arguments):
        return subprocess.run(["git", *arguments], cwd=self.repo, text=True,
                              check=True, capture_output=True).stdout.strip()

    def commit(self, path, content):
        (self.repo / path).write_text(content, encoding="utf-8")
        self.git("add", "--", path)
        self.git("commit", "--quiet", "-m", "Fixture update")

    def run_handoff(self, release=None):
        env = dict(os.environ, GITHUB_SERVER_URL="https://github.example",
                   GITHUB_REPOSITORY="fixture/example", GITHUB_STEP_SUMMARY=str(self.summary))
        return subprocess.run(["bash", str(SCRIPT), self.main if release is None else release],
                              cwd=self.repo, env=env, text=True, capture_output=True)

    def remote_ref(self, ref):
        return self.git("ls-remote", "--refs", "origin", ref).split("\t", 1)[0]

    def test_prepares_exact_history_only_branch_and_keeps_it_when_develop_advances(self):
        develop = self.git("rev-parse", "develop")
        result = self.run_handoff()
        self.assertEqual(result.returncode, 0, result.stderr)
        branch_tip = self.remote_ref(self.ref)
        self.assertEqual(self.git("rev-list", "--parents", "-n", "1", branch_tip),
                         f"{branch_tip} {develop} {self.main}")
        self.assertEqual(self.git("rev-parse", f"{branch_tip}^{{tree}}"), self.git("rev-parse", "develop^{tree}"))
        self.assertEqual(self.remote_ref("refs/heads/develop"), develop)
        self.assertEqual(self.remote_ref("refs/heads/main"), self.main)
        summary = self.summary.read_text()
        self.assertIn(f"https://github.example/fixture/example/compare/develop...{self.branch}?expand=1", summary)
        self.assertIn(branch_tip, summary)
        self.assertIn("using a merge commit", summary)

        self.git("checkout", "--quiet", "develop")
        self.commit("features", "new work")
        self.git("push", "--quiet", "origin", "develop")
        self.git("checkout", "--quiet", "main")
        result = self.run_handoff()
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertEqual(self.remote_ref(self.ref), branch_tip)
        self.git("checkout", "--quiet", "develop")
        before = self.git("rev-parse", "HEAD^{tree}")
        self.git("merge", "--no-ff", "-m", "Reconcile release history", branch_tip)
        self.assertEqual(self.git("rev-parse", "HEAD^{tree}"), before)
        self.git("merge-base", "--is-ancestor", self.main, "HEAD")

    def test_skips_already_reconciled_release(self):
        self.git("push", "--quiet", "origin", "main:refs/heads/develop")
        result = self.run_handoff()
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertIn("already contains", result.stdout)
        self.assertFalse(self.summary.exists())
        self.assertEqual(self.remote_ref(self.ref), "")

    def test_skips_a_superseded_release(self):
        self.commit("new-release", "supersedes")
        self.git("push", "--quiet", "origin", "main")
        self.git("checkout", "--quiet", "--detach", self.main)
        result = self.run_handoff()
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertIn("newer main commit", result.stdout)
        self.assertEqual(self.remote_ref(self.ref), "")

    def test_refuses_a_checkout_mismatch(self):
        result = self.run_handoff(self.git("rev-parse", "develop"))
        self.assertNotEqual(result.returncode, 0)
        self.assertIn("checkout does not match", result.stderr)
        self.assertEqual(self.remote_ref(self.ref), "")

    def test_refuses_a_clean_merge_that_would_add_release_content(self):
        self.commit("main-only", "change absent from develop")
        self.git("push", "--quiet", "origin", "main")
        main = self.git("rev-parse", "HEAD")
        result = self.run_handoff(main)
        self.assertNotEqual(result.returncode, 0)
        self.assertIn("would change develop content", result.stderr)
        self.assertEqual(self.remote_ref(f"refs/heads/chore/back-merge-main-{main}"), "")

    def test_refuses_conflicting_content(self):
        self.commit("features", "main changed the file")
        self.git("push", "--quiet", "origin", "main")
        main = self.git("rev-parse", "HEAD")
        self.git("checkout", "--quiet", "develop")
        self.commit("features", "develop changed the same file")
        self.git("push", "--quiet", "origin", "develop")
        self.git("checkout", "--quiet", "main")
        result = self.run_handoff(main)
        self.assertNotEqual(result.returncode, 0)
        self.assertIn("needs conflict resolution", result.stderr)
        self.assertEqual(self.remote_ref(f"refs/heads/chore/back-merge-main-{main}"), "")

    def test_rejects_a_preexisting_branch_without_the_expected_parents(self):
        self.git("push", "--quiet", "origin", f"main:{self.ref}")
        result = self.run_handoff()
        self.assertNotEqual(result.returncode, 0)
        self.assertIn("no longer matches the history-only handoff", result.stderr)
        self.assertEqual(self.remote_ref(self.ref), self.main)
        self.assertFalse(self.summary.exists())


class ReleaseHandoffWorkflowTests(unittest.TestCase):
    def test_preparation_only_uses_contents_and_follows_confirmed_publication(self):
        publish = yaml.safe_load((GITHUB / "workflows" / "publish.yml").read_text())
        handoff = publish["jobs"]["back-merge"]
        self.assertEqual(handoff["permissions"], {"contents": "write"})
        self.assertEqual(handoff["needs"], "release")
        self.assertIn('prepare-release-back-merge.sh "$GITHUB_SHA"', handoff["steps"][-1]["run"])


if __name__ == "__main__":
    unittest.main()
