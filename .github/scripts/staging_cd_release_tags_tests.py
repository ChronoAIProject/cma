"""Immutable object acceleration must never cache a mutable release decision."""
import hashlib
import json
import subprocess
import tempfile
import unittest
from pathlib import Path
from unittest.mock import Mock, patch

from staging_cd_common import Refusal
from staging_cd_github import GitHub
from staging_cd_release_tags import MAX_TAG, local_tag_reader, read_local_tag
from staging_cd_reservations import Reservations, next_reservation


class ReleaseTagTests(unittest.TestCase):
    def setUp(self):
        temp = tempfile.TemporaryDirectory()
        self.addCleanup(temp.cleanup)
        self.root = Path(temp.name)
        self.git("init", "--quiet")
        self.git("config", "user.name", "Fixture")
        self.git("config", "user.email", "fixture@example.invalid")
        self.commit = self.git("commit-tree", self.git("mktree"), "-m", "Synthetic root")
        self.tree = self.git("rev-parse", self.commit + "^{tree}")
        self.api = GitHub("fixture", local_tags=local_tag_reader("web", self.root))
        self.store = Reservations(self.api, "fixture", self.root)

    def git(self, *args, data=None):
        return subprocess.run(["git", *args], cwd=self.root, input=data, capture_output=True,
                              check=True, timeout=5, text=True).stdout.strip()

    def pending(self, owner):
        value = next_reservation("web", self.commit, self.tree, 1, "patch", owner, 1, [], [])
        name = "cma-web/pending/v" + value["version"]
        raw = (f"object {self.commit}\ntype commit\ntag {name}\n"
               "tagger Fixture <fixture@example.invalid> 0 +0000\n\n" + json.dumps(value) + "\n")
        oid = self.git("mktag", data=raw)
        return value, {"ref": "refs/tags/" + name, "object": {"type": "tag", "sha": oid}}

    def test_local_body_matches_and_each_ref_inventory_is_remote(self):
        first, ref = self.pending(1)
        other, replacement = self.pending(2)
        def result(value):
            return Mock(returncode=0, stdout=json.dumps(value))
        with patch.object(self.api, "runner", side_effect=map(result, ([ref], [ref], [replacement], []))) as remote:
            self.assertEqual(self.store.list("web"), [first])
            self.assertEqual(self.store.list("web"), [first])
            self.assertEqual(self.store.list("web"), [other])
            self.assertEqual(self.store.list("web"), [])
        self.assertEqual([c.args[0][-1].split("/cma/")[1] for c in remote.call_args_list],
                         ["git/matching-refs/tags/cma-web/pending/v"] * 4)
        self.assertIsNone(local_tag_reader("am", self.root))

    def test_missing_signed_non_tag_and_oversize_defer_to_github(self):
        prefix = f"object {self.commit}\ntype commit\ntag sample\ntagger F <f@example.invalid> 0 +0000\n\n"
        signed = self.git("mktag", data=prefix + "message\n-----BEGIN PGP SIGNATURE-----\n")
        large = self.git("mktag", data=prefix + "x" * MAX_TAG)
        for oid in (signed, large, self.commit, "0" * 40):
            with self.subTest(oid=oid), patch.object(self.api, "runner", return_value=Mock(returncode=0, stdout='{"remote": true}')) as remote:
                self.assertEqual(self.api.request("git/tags/" + oid), {"remote": True})
                self.assertEqual(remote.call_args.args[0][-1], "repos/ChronoAIProject/cma/git/tags/" + oid)

    def test_lookup_failure_does_not_become_authority(self):
        path = "git/tags/" + "a" * 40
        with patch.object(self.api, "runner", side_effect=[Refusal("unavailable"), Mock(returncode=0, stdout='{"fresh": true}')]):
            with self.assertRaisesRegex(Refusal, "unavailable"):
                self.api.request(path)
            self.assertEqual(self.api.request(path), {"fresh": True})
        with patch.object(self.api, "runner", side_effect=[
                Mock(returncode=0, stdout="HTTP/2 200 OK\n\n{}"), Mock(returncode=0, stdout="{}")]) as remote:
            self.api.request(path, allow_not_found=True)
            self.api.request(path, {"tag": "synthetic"})
            self.assertEqual(remote.call_count, 2)

    def test_mismatched_bytes_and_invalid_id_refuse(self):
        oid = hashlib.sha1(b"tag 2\0ok").hexdigest()
        with patch("staging_cd_release_tags.subprocess.run", side_effect=[
                Mock(stdout=f"{oid} tag 2\n".encode()), Mock(stdout=b"no")]):
            with self.assertRaisesRegex(Refusal, "advertised ID"):
                read_local_tag(oid, self.root)
        with self.assertRaisesRegex(Refusal, "Invalid release tag"):
            read_local_tag("refs/heads/develop", self.root)

    def test_local_reader_does_not_follow_replace_refs(self):
        first, ref = self.pending(1)
        _, replacement = self.pending(2)
        oid = ref["object"]["sha"]
        self.git("replace", oid, replacement["object"]["sha"])
        self.assertEqual(json.loads(read_local_tag(oid, self.root)["message"]), first)


if __name__ == "__main__":
    unittest.main()
