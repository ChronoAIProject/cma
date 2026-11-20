"""Count GitHub API calls of one AM release lane holding 32 finals and 84 pending tags.

The lane mirrors the measured develop state: one current reservation, 83
burned reservations below the newest final, and a complete job checkout.
A disposable bare remote stands in for GitHub; every API read is counted.
"""
import collections
import contextlib
import io
import json
import os
import subprocess
import tempfile
import unittest
from pathlib import Path
from unittest.mock import Mock, patch

import staging_cd_finalize
import staging_cd_release_ownership
from staging_cd_github import GitHub
from staging_cd_receipts import canonical, final_tag_text
from staging_cd_receipts_tests import fixture_receipt
from staging_cd_release_tags import local_tag_reader
from staging_cd_reservations import Reservations, claim_ref, pending_name, validate_reservation

REMOTE = "https://github.com/ChronoAIProject/cma.git"
FINALS, BURNED = 32, 83
RUN_ID = 9000


class Lane:
    """A bare remote with AM's tags and a full clone acting as the job checkout."""

    def __init__(self, root, *, finals=FINALS, burned=BURNED):
        self.root, self.remote, self.checkout = root, root / "remote.git", root / "checkout"
        self.env = dict(os.environ, GIT_CONFIG_NOSYSTEM="1", GIT_CONFIG_GLOBAL=os.devnull,
                        GIT_AUTHOR_NAME="Fixture", GIT_AUTHOR_EMAIL="fixture@example.invalid",
                        GIT_COMMITTER_NAME="Fixture", GIT_COMMITTER_EMAIL="fixture@example.invalid")
        self.calls = collections.Counter()
        self.git("init", "--quiet", "--bare", str(self.remote), cwd=root)
        self.tree = self.bare("mktree", data="")
        count = finals + burned
        # Finals take the newest versions below the current one, as on develop.
        final_versions = set(range(count - finals + 1, count + 1))
        sources, refs = [], []
        for index in range(1, count + 2):
            parent = ["-p", sources[-1]] if sources else []
            sources.append(self.bare("commit-tree", self.tree, *parent, data=f"Merge {index}\n"))
            identity = {"component": "am", "version": f"0.10.{index}", "previous": f"0.10.{index - 1}",
                        "source": sources[-1], "tree": self.tree, "pr": index, "bump": "patch",
                        "run_id": RUN_ID if index == count + 1 else 1000 + index, "run_attempt": 1}
            if index in final_versions:
                receipt = fixture_receipt(**identity)
                refs.append(("refs/tags/" + receipt["tag"], self.bare("mktag", data=final_tag_text(receipt))))
                continue
            value = validate_reservation(dict(identity, schema="cma-version-reservation-v1"))
            oid = self.bare("mktag", data=f"object {value['source']}\ntype commit\ntag {pending_name(value)}\n"
                                          "tagger Fixture <fixture@example.invalid> 0 +0000\n\n"
                                          + canonical(value) + "\n")
            refs += [("refs/tags/" + pending_name(value), oid), (claim_ref(value), oid)]
            self.current = value
        refs.append(("refs/heads/develop", sources[-1]))
        self.bare("update-ref", "--stdin", data="".join(f"create {ref} {oid}\n" for ref, oid in refs))
        # A complete checkout, like actions/checkout with fetch-depth 0, holds every tag object.
        self.git("clone", "--quiet", str(self.remote), str(self.checkout), cwd=root)
        self.receipt = fixture_receipt(**{key: value for key, value in self.current.items() if key != "schema"})

    def git(self, *args, cwd=None, data=None):
        return subprocess.run(["git", *args], cwd=cwd or self.checkout, env=self.env, input=data, text=True,
                              capture_output=True, check=True, timeout=30).stdout.strip()

    def bare(self, *args, data=None):
        return self.git("--git-dir", str(self.remote), *args, cwd=self.root, data=data)

    def run(self, args, **kwargs):
        """Real Git against the bare remote in place of github.com."""
        if args[0] == "git":
            # A fixture must never write into the repository running the tests.
            cwd = Path(kwargs.get("cwd") or Path.cwd()).resolve()
            assert cwd.is_relative_to(self.root.resolve()), "Git ran outside the fixture"
        if args[0] == "git" and "push" in args:
            self.calls["git push"] += 1
        elif args[0] == "git" and REMOTE in args:
            self.calls["git " + next(arg for arg in args if arg in ("fetch", "ls-remote"))] += 1
        return subprocess.run([str(self.remote) if arg == REMOTE else arg for arg in args], **kwargs)

    def api(self, args, **kwargs):
        if args[0] != "gh":
            return self.run(args, **kwargs)
        path = args[-1].removeprefix("repos/ChronoAIProject/cma/")
        self.calls["api " + ("git/tags/<oid>" if path.startswith("git/tags/") else path)] += 1
        if path == "git/ref/heads/develop":
            value = {"ref": "refs/heads/develop",
                     "object": {"type": "commit", "sha": self.bare("rev-parse", "refs/heads/develop")}}
        elif path.startswith("git/matching-refs/"):
            prefix = "refs/" + path.removeprefix("git/matching-refs/")
            rows = self.bare("for-each-ref", "--format=%(refname) %(objectname) %(objecttype)").splitlines()
            value = [{"ref": ref, "object": {"sha": oid, "type": kind}}
                     for ref, oid, kind in map(str.split, rows) if ref.startswith(prefix)]
        else:
            oid = path.removeprefix("git/tags/")
            header, message = self.bare("cat-file", "tag", oid).split("\n\n", 1)
            fields = dict(line.split(" ", 1) for line in header.splitlines())
            value = {"sha": oid, "tag": fields["tag"], "message": message + "\n",
                     "object": {"type": fields["type"], "sha": fields["object"]}}
        return subprocess.CompletedProcess(args, 0, "HTTP/2 200 OK\n\n" + json.dumps(value), "")

    def github(self, token, **kwargs):
        return GitHub(token, runner=self.api, **kwargs)

    def reservations(self, api, token, root, **kwargs):
        return Reservations(api, token, root, self.run, **kwargs)

    def measure(self, operation):
        # Each job runs from its checkout, where develop ancestry is fetched.
        self.calls.clear()
        cwd = Path.cwd()
        os.chdir(self.checkout)
        try:
            operation()
        finally:
            os.chdir(cwd)
        return {key: count for key, count in sorted(self.calls.items())}

    def ownership(self):
        env = {"CD_COMPONENT": "am", "CD_TASKS": self.tasks(), "GH_TOKEN": "fixture",
               "GITHUB_RUN_ID": str(RUN_ID), "GITHUB_RUN_ATTEMPT": "1"}
        with patch.object(staging_cd_release_ownership, "event_identity"):
            assert staging_cd_release_ownership.owned(env, self.checkout, github=self.github) is True

    def publication(self, local_tags):
        # The pre-build check, then the three checks around the registry push.
        store = Reservations(self.github("fixture", local_tags=local_tags), "fixture", self.checkout, self.run)
        for started in (False, True, True, True):
            assert store.require_current(self.current, build_started=started) is True

    def finalization(self):
        receipts, output = self.root / "receipts.json", self.root / "output"
        receipts.write_text(json.dumps([self.receipt]))
        env = {"GITHUB_EVENT_NAME": "push", "GITHUB_RUN_ID": str(RUN_ID), "GITHUB_RUN_ATTEMPT": "1",
               "GH_TOKEN": "fixture", "CD_COMPONENT": "am", "CD_TASKS": self.tasks(),
               "CD_RECEIPTS_PATH": str(receipts), "GITHUB_OUTPUT": str(output), "PATH": os.environ["PATH"]}
        stdout = io.StringIO()
        with patch.dict(os.environ, env, clear=True), patch.object(staging_cd_finalize, "event_identity"), \
                patch.object(staging_cd_finalize, "GitHub", side_effect=self.github), \
                patch.object(staging_cd_finalize, "Registry", return_value=Mock(exists=lambda *_: True)), \
                patch.object(staging_cd_finalize, "Reservations", side_effect=self.reservations), \
                contextlib.redirect_stdout(stdout):
            status = staging_cd_finalize.main()
        assert status == 0, stdout.getvalue()
        return stdout.getvalue()

    def tasks(self):
        return json.dumps([{"component": "am", "mode": "publish", "reservation": self.current}])


def api_calls(counts):
    return sum(count for key, count in counts.items() if key.startswith("api "))


class ReleaseCallCountTests(unittest.TestCase):
    def setUp(self):
        temp = tempfile.TemporaryDirectory()
        self.addCleanup(temp.cleanup)
        self.lane = Lane(Path(temp.name))

    def test_am_release_reads_a_constant_number_of_api_calls_and_no_annotation(self):
        lane = self.lane
        ownership = lane.measure(lane.ownership)
        publication = lane.measure(lambda: lane.publication(local_tag_reader(lane.checkout)))
        report = []
        finalization = lane.measure(lambda: report.append(lane.finalization()))
        for counts in (ownership, publication, finalization):
            self.assertNotIn("api git/tags/<oid>", counts)
        # Develop ancestry once per process; both live inventories at every check.
        self.assertEqual(api_calls(ownership), 3)
        self.assertEqual(api_calls(publication), 9)
        self.assertLessEqual(api_calls(finalization), 12)
        self.assertEqual(finalization["git push"], 3)
        self.assertIn("PRUNE finalize component=am result=complete removed=83\n", report[0])
        # Every burned reservation below the new final left with its claim; the next release reads less.
        # The new final itself backs its own reservation's claim, as before.
        self.assertEqual(lane.bare("for-each-ref", "refs/tags/cma-agent-manager/pending/"), "")
        self.assertEqual(lane.bare("for-each-ref", "--format=%(refname)", "refs/tags/cma-allocation/am/"),
                         claim_ref(lane.current))
        self.assertEqual(len(lane.bare("for-each-ref", "refs/tags/cma-agent-manager/v*").splitlines()), FINALS + 1)

    def test_counts_do_not_grow_with_the_number_of_tags(self):
        with tempfile.TemporaryDirectory() as directory:
            small = Lane(Path(directory), finals=2, burned=3)
            counts = [lane.measure(lambda lane=lane: lane.publication(local_tag_reader(lane.checkout)))
                      for lane in (small, self.lane)]
        self.assertEqual(counts[0], counts[1])


if __name__ == "__main__":
    unittest.main()
