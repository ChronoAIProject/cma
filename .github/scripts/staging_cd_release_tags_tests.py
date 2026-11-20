"""Immutable object acceleration must never cache a mutable release decision."""
import hashlib
import json
import subprocess
import tempfile
import unittest
from pathlib import Path
from unittest.mock import Mock, patch

from staging_cd_common import Refusal
from staging_cd_receipts import annotation, final_tag_text
from staging_cd_receipts_tests import fixture_receipt, fixture_runtime_receipt
from staging_cd_github import GitHub
from staging_cd_release_tags import MAX_TAG, LocalTags, local_tag_reader, read_local_tag
from staging_cd_reservations import Reservations, next_reservation, pending_name
from staging_cd_versions import DEPLOYABLES

LANES = sorted(DEPLOYABLES)


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
        self.api = GitHub("fixture", local_tags=LocalTags(self.root))
        self.store = Reservations(self.api, "fixture", self.root)

    def git(self, *args, data=None):
        return subprocess.run(["git", *args], cwd=self.root, input=data, capture_output=True,
                              check=True, timeout=5, text=True).stdout.strip()

    def pending(self, owner, component="web", earlier=()):
        value = next_reservation(component, self.commit, self.tree, 1, "patch", owner, 1, [], list(earlier))
        name = pending_name(value)
        raw = (f"object {self.commit}\ntype commit\ntag {name}\n"
               "tagger Fixture <fixture@example.invalid> 0 +0000\n\n" + json.dumps(value) + "\n")
        oid = self.git("mktag", data=raw)
        return value, {"ref": "refs/tags/" + name, "object": {"type": "tag", "sha": oid}}

    def final(self, component):
        make = fixture_runtime_receipt if component.startswith("runtime") else fixture_receipt
        receipt = make(component=component, source=self.commit, tree=self.tree)
        oid = self.git("mktag", data=final_tag_text(receipt))
        return receipt, {"ref": "refs/tags/" + receipt["tag"], "object": {"type": "tag", "sha": oid}}

    def test_every_lane_reads_annotations_from_its_checkout_after_each_live_inventory(self):
        for lane in LANES:
            with self.subTest(lane=lane):
                api = GitHub("fixture", local_tags=local_tag_reader(self.root))
                store = Reservations(api, "fixture", self.root)
                value, pending = self.pending(1, lane)
                receipt, final = self.final(lane)
                rows = [Mock(returncode=0, stdout=json.dumps(rows)) for rows in ([pending], [final]) * 2]
                with patch.object(api, "runner", side_effect=rows) as remote, \
                        patch.object(api, "history"), patch.object(api, "history_containing"):
                    for _ in range(2):
                        self.assertEqual(store.list(lane), [value])
                        record, = api.tags(lane)
                        self.assertEqual((record["oid"], record["receipt"]), (final["object"]["sha"], receipt))
                # Both inventories are re-read at every check; no annotation goes to the API.
                self.assertEqual([c.args[0][-1].split("/cma/")[1] for c in remote.call_args_list],
                                 ["git/matching-refs/tags/" + DEPLOYABLES[lane] + suffix
                                  for suffix in ("/pending/v", "/v")] * 2)

    def test_one_inventory_reads_every_local_annotation_in_two_git_calls(self):
        values, rows, calls = [], [], []
        for owner in range(1, 41):
            value, row = self.pending(owner, earlier=values)
            values.append(value)
            rows.append(row)
        def runner(args, **kwargs):
            calls.append(args[1:])
            return subprocess.run(args, **kwargs)
        api = GitHub("fixture", local_tags=LocalTags(self.root, runner=runner))
        with patch.object(api, "runner", return_value=Mock(returncode=0, stdout=json.dumps(rows))) as remote:
            self.assertEqual(Reservations(api, "fixture", self.root).list("web"), values)
        self.assertEqual(remote.call_count, 1)
        self.assertEqual(calls, [["--no-replace-objects", "--no-lazy-fetch", "cat-file", mode]
                                 for mode in ("--batch-check", "--batch")])

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

    def test_missing_signed_non_tag_and_oversize_defer_to_github(self):
        prefix = f"object {self.commit}\ntype commit\ntag sample\ntagger F <f@example.invalid> 0 +0000\n\n"
        signed = self.git("mktag", data=prefix + "message\n-----BEGIN PGP SIGNATURE-----\n")
        large = self.git("mktag", data=prefix + "x" * MAX_TAG)
        for oid in (signed, large, self.commit, "0" * 40):
            with self.subTest(oid=oid), patch.object(self.api, "runner", return_value=Mock(returncode=0, stdout=json.dumps({"sha": oid}))) as remote:
                self.assertEqual(self.api.request("git/tags/" + oid), {"sha": oid})
                self.assertEqual(remote.call_args.args[0][-1], "repos/ChronoAIProject/cma/git/tags/" + oid)

    def test_missing_or_mismatched_remote_oid_stops_cleanup(self):
        receipt = fixture_receipt()
        oid = "a" * 40
        row = {"ref": "refs/tags/" + receipt["tag"], "object": {"type": "tag", "sha": oid}}
        tag = {"tag": receipt["tag"], "message": json.dumps(receipt),
               "object": {"type": "commit", "sha": receipt["source"]}}
        for response in (tag, dict(tag, sha="b" * 40)):
            results = [Mock(returncode=0, stdout=json.dumps(value)) for value in ([row], response)]
            with self.subTest(response_oid=response.get("sha")), patch.object(self.api, "runner", side_effect=results), \
                    patch.object(self.store, "command") as write:
                with self.assertRaisesRegex(Refusal, "tag object differs from advertised ID"):
                    self.store.remove_after_publication(receipt, oid)
                write.assert_not_called()

    def test_historical_signed_tag_uses_authenticated_object_fields(self):
        receipt = fixture_receipt(source=self.commit, tree=self.tree)
        name = receipt["tag"]
        header = f"object {self.commit}\ntype commit\ntag {name}\ntagger Historical <f@example.invalid> 0 +0000\n\n"
        oid = self.git("mktag", data=header + annotation(receipt) + "-----BEGIN PGP SIGNATURE-----\n")
        ref = {"ref": "refs/tags/" + name, "object": {"type": "tag", "sha": oid}}
        obj = {"tag": name, "sha": oid, "object": {"type": "commit", "sha": self.commit},
               "message": annotation(receipt)}
        results = [Mock(returncode=0, stdout=json.dumps(value)) for value in ([ref], obj)]
        with patch.object(self.api, "runner", side_effect=results) as remote, patch.object(self.api, "history"):
            record, = self.api.tags("web")
        self.assertEqual((record["oid"], record["receipt"]), (oid, receipt))
        self.assertTrue(remote.call_args.args[0][-1].endswith("git/tags/" + oid))

    def test_lookup_failure_does_not_become_authority(self):
        path = "git/tags/" + "a" * 40
        with patch.object(self.api, "runner", side_effect=[Refusal("unavailable"), Mock(returncode=0, stdout=json.dumps({"sha": "a" * 40}))]):
            with self.assertRaisesRegex(Refusal, "unavailable"):
                self.api.request(path)
            self.assertEqual(self.api.request(path), {"sha": "a" * 40})
        with patch.object(self.api, "runner", side_effect=[
                Mock(returncode=0, stdout="HTTP/2 200 OK\n\n" + json.dumps({"sha": "a" * 40})), Mock(returncode=0, stdout="{}")]) as remote:
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
