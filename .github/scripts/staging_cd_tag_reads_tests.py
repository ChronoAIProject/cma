"""Release checks re-read live refs but each immutable tag object at most once."""
import contextlib
import io
import json
import os
import tempfile
import unittest
from pathlib import Path
from unittest.mock import Mock, patch

import staging_cd_finalize
import staging_cd_publish
import staging_cd_release_ownership
import staging_cd_resolve
from staging_cd_github import GitHub
from staging_cd_receipts import annotation
from staging_cd_receipts_tests import fixture_receipt
from staging_cd_reservations import next_reservation


class Stop(Exception):
    """Ends an entry point once its GitHub client has been constructed."""


class TagReadTests(unittest.TestCase):
    def test_repeated_checks_read_each_tag_object_once_and_every_inventory_again(self):
        receipt = fixture_receipt()
        oid = "a" * 40
        responses = {
            "git/matching-refs/tags/cma-web/v": [{"ref": "refs/tags/" + receipt["tag"],
                                                 "object": {"type": "tag", "sha": oid}}],
            "git/tags/" + oid: {"sha": oid, "tag": receipt["tag"], "message": annotation(receipt),
                                "object": {"type": "commit", "sha": receipt["source"]}},
        }
        api = GitHub("fixture")
        def runner(args, **_kwargs):
            return Mock(returncode=0, stdout=json.dumps(responses[args[-1].split("/cma/", 1)[1]]))
        with patch.object(api, "runner", side_effect=runner) as remote, patch.object(api, "history"):
            for _ in range(3):
                record, = api.tags("web")
                self.assertEqual(record["receipt"], receipt)
            # A caller mutating a returned object cannot change the remembered one.
            api.request("git/tags/" + oid)["message"] = "mutated"
            self.assertEqual(api.request("git/tags/" + oid)["message"], annotation(receipt))
        self.assertEqual([c.args[0][-1].split("/cma/", 1)[1] for c in remote.call_args_list],
                         ["git/matching-refs/tags/cma-web/v", "git/tags/" + oid,
                          "git/matching-refs/tags/cma-web/v", "git/matching-refs/tags/cma-web/v"])

    def test_every_release_entry_point_reads_tag_objects_from_its_checkout(self):
        # AM is the lane whose finalization spent minutes on per-object API reads.
        reservation = next_reservation("am", "a" * 40, "b" * 40, 1, "patch", 7, 1, [], [])
        with tempfile.TemporaryDirectory() as directory:
            env = {"GH_TOKEN": "fixture", "GITHUB_EVENT_NAME": "push", "CD_COMPONENT": "am",
                   "GITHUB_RUN_ID": "7", "GITHUB_RUN_ATTEMPT": "1", "CD_RESOLVE_MODE": "verify",
                   "CD_RECEIPTS_PATH": str(Path(directory) / "receipts.json"),
                   "CD_TASKS": json.dumps([{"component": "am", "mode": "publish", "reservation": reservation}])}
            for module in (staging_cd_resolve, staging_cd_finalize, staging_cd_publish):
                with self.subTest(entry=module.__name__):
                    github = Mock(side_effect=Stop())
                    with patch.dict(os.environ, env, clear=True), patch.object(module, "GitHub", github), \
                            patch.object(module, "Registry"), \
                            patch.object(module, "event_identity", create=True), \
                            contextlib.redirect_stdout(io.StringIO()):
                        self.assertNotEqual(module.main(), 0)
                    self.assertEqual(getattr(github.call_args.kwargs.get("local_tags"), "root", None), Path.cwd())
            store = Mock()
            with patch.object(staging_cd_release_ownership, "event_identity"):
                self.assertTrue(staging_cd_release_ownership.owned(env, Path(directory), reservations=store))
            self.assertEqual(getattr(store.call_args.args[0].local_tags, "root", None), Path(directory))


if __name__ == "__main__":
    unittest.main()
