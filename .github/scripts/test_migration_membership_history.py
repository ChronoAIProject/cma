"""A collector must not accept foreign incarnations or relabel old receipts."""
import json
import os
from pathlib import Path
import tempfile
import unittest
from unittest.mock import Mock

from migration_catalog import STAGES
from migration_membership_documents import capture
from migration_membership_history import read_bounded
from test_migration_membership_documents import ready_document
from test_migration_membership_validation import snapshot, row, pod
from types import SimpleNamespace


class HistoryTests(unittest.TestCase):
    def campaign(self, directory):
        return SimpleNamespace(output=Path(directory), active_reader={'pod': pod(), 'source': 'old'},
                               reader_pods=Mock(return_value=ready_document()),
                               kubectl=Mock(side_effect=['', '', json.dumps(snapshot())]))

    def advance(self, campaign, uid='new', source='new', ip='10.0.0.3', now=25000, rows=None):
        selected = pod(uid, ip)
        doc = ready_document()
        doc['items'][0]['metadata'].update(uid=uid, name=uid)
        doc['items'][0]['status']['podIP'] = ip
        campaign.active_reader = {'pod': selected, 'source': source}
        campaign.reader_pods.return_value = doc
        if rows is None:
            rows = [row(uid, ip, now + 15000)]
        campaign.kubectl.reset_mock()
        campaign.kubectl.side_effect = ['', '', json.dumps(snapshot(rows, now))]

    def first(self, campaign):
        return capture(campaign, 'n', 'mongo', STAGES[0], 'old', 'old-image')

    def test_initial_snapshot_cannot_hide_an_unknown_expired_member(self):
        with tempfile.TemporaryDirectory() as directory:
            campaign = self.campaign(directory)
            campaign.kubectl.side_effect = ['', '', json.dumps(snapshot([
                row(), row('unknown', '10.0.0.9', 999)]))]
            with self.assertRaisesRegex(ValueError, 'unexplained process incarnation'):
                self.first(campaign)

    def test_later_snapshot_cannot_add_an_unexplained_draining_member(self):
        with tempfile.TemporaryDirectory() as directory:
            campaign = self.campaign(directory); self.first(campaign)
            self.advance(campaign, rows=[row('new', '10.0.0.3', 40000),
                                        {**row('unknown', '10.0.0.9', 40000), 'draining': True}])
            with self.assertRaisesRegex(ValueError, 'unexplained process incarnation'):
                capture(campaign, 'n', 'mongo', STAGES[1], 'new', 'new-image')

    def test_stage_and_booted_source_are_verified_before_creating_a_job(self):
        for stage, source in [(STAGES[1], 'old'), (STAGES[0], 'wrong'), ('unknown', 'old')]:
            with tempfile.TemporaryDirectory() as directory:
                campaign = self.campaign(directory)
                with self.assertRaises(ValueError):
                    capture(campaign, 'n', 'mongo', stage, source, 'old-image')
                campaign.kubectl.assert_not_called()

    def test_successful_stage_cannot_be_repeated_or_mutated_through_return_value(self):
        with tempfile.TemporaryDirectory() as directory:
            campaign = self.campaign(directory)
            receipt = self.first(campaign)
            receipt['source'] = 'caller-mutated'
            receipt['membership_pod']['uid'] = 'caller-mutated'
            campaign.kubectl.reset_mock()
            with self.assertRaisesRegex(ValueError, 'skipped or repeated'):
                self.first(campaign)
            campaign.kubectl.assert_not_called()
            self.advance(campaign)
            second = capture(campaign, 'n', 'mongo', STAGES[1], 'new', 'new-image')
            self.assertEqual(second['comparisons'][0]['from_stage'], STAGES[0])

    def test_evidence_reader_refuses_missing_symlink_fifo_and_oversized_files(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            regular = root / 'regular'; regular.write_bytes(b'{}')
            self.assertEqual(read_bounded(regular), b'{}')
            link = root / 'link'; link.symlink_to(regular)
            fifo = root / 'fifo'; os.mkfifo(fifo)
            huge = root / 'huge'; huge.write_bytes(b'x' * 65537)
            for path in [root / 'missing', link, fifo, huge, root]:
                with self.subTest(path=path), self.assertRaises(ValueError):
                    read_bounded(path)

    def test_changed_prior_binding_cannot_relabel_writer_or_pod(self):
        for field, value in [('source', 'unrelated'), ('image', 'different-image'),
                             ('path', '../outside.json'), ('stage', STAGES[2]),
                             ('serving_pod', pod('other'))]:
            with self.subTest(field=field), tempfile.TemporaryDirectory() as directory:
                campaign = self.campaign(directory); self.first(campaign)
                path = campaign.output / 'membership-old-write.binding.json'
                receipt = json.loads(path.read_text()); receipt[field] = value
                path.write_text(json.dumps(receipt))
                self.advance(campaign)
                with self.assertRaisesRegex(ValueError, 'prior membership binding changed'):
                    capture(campaign, 'n', 'mongo', STAGES[1], 'new', 'new-image')

    def test_image_pair_must_match_when_the_same_reader_returns(self):
        with tempfile.TemporaryDirectory() as directory:
            campaign = self.campaign(directory); self.first(campaign)
            self.advance(campaign)
            capture(campaign, 'n', 'mongo', STAGES[1], 'new', 'new-image')
            self.advance(campaign, now=29000)
            with self.assertRaisesRegex(ValueError, 'reader artifact changed'):
                capture(campaign, 'n', 'mongo', STAGES[2], 'new', 'unrelated-image')
            campaign.kubectl.assert_not_called()

    def test_complete_four_stage_history_keeps_exact_membership_pod(self):
        with tempfile.TemporaryDirectory() as directory:
            campaign = self.campaign(directory); self.first(campaign)
            self.advance(campaign)
            capture(campaign, 'n', 'mongo', STAGES[1], 'new', 'new-image')
            self.advance(campaign, now=29000)
            capture(campaign, 'n', 'mongo', STAGES[2], 'new', 'new-image')
            self.advance(campaign, uid='rollback', source='old', ip='10.0.0.4', now=50000)
            result = capture(campaign, 'n', 'mongo', STAGES[3], 'old', 'old-image')
            # The runner's generic serving_pod field is narrower and overwrites
            # that key. Keep the richer membership identity under its own key.
            merged = {**result, 'serving_pod': {'uid': 'rollback'}}
            self.assertEqual(merged['membership_pod']['ip'], '10.0.0.4')
            self.assertEqual(len(result['comparisons']), 3)
            self.assertEqual(result['application_cross_reader_compatibility'], 'NOT_PROVEN')


if __name__ == '__main__':
    unittest.main()
