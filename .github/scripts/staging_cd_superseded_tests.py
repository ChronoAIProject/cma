"""Newest-source scheduling preserves burned versions and fences stale writers."""
import copy
import io
import json
import os
import tempfile
import unittest
from contextlib import ExitStack, redirect_stdout
from pathlib import Path
from unittest.mock import Mock, patch

import staging_cd_publish as publication
from staging_cd_allocate import allocate_queue
from staging_cd_common import Refusal
from staging_cd_history import DevelopHistory
from staging_cd_publish_tests import PublicationTests
from staging_cd_receipts_tests import fixture_identity
from staging_cd_reservations import Reservations, next_reservation
from staging_cd_versions import version_tuple


class SupersededTests(unittest.TestCase):
    def fixture(self):
        sources = [letter * 40 for letter in 'abcd']
        pending = []
        for index, source in enumerate(sources[:3]):
            pending.append(next_reservation('am', source, 'e' * 40, index + 1, 'patch',
                                            index + 10, 1, [], pending))
        history = DevelopHistory(sources[-1], list(reversed(sources)))
        api, store = Mock(), Mock()
        api.tags.return_value = []
        api.history.return_value = history
        prs = {source: {'number': i + 1, 'merged': True, 'merged_at': 'fixture', 'merge_commit_sha': source,
                       'base': {'ref': 'develop', 'repo': {'full_name': 'ChronoAIProject/cma'}},
                       'labels': [{'name': 'version-upgrade:patch'}]} for i, source in enumerate(sources)}
        def request(path, **_kwargs):
            if path.startswith('actions/runs/'):
                return {'id': int(path.split('/')[2]), 'run_attempt': 1, 'event': 'push',
                        'head_branch': 'develop', 'repository': {'full_name': 'ChronoAIProject/cma'},
                        'status': 'completed'}
            if path.startswith('commits/'):
                return [prs[path.split('/')[1]]]
            return next(pr for pr in prs.values() if pr['number'] == int(path.split('/')[1]))
        api.request.side_effect = request
        store.list.side_effect = lambda: copy.deepcopy(pending)
        store.insert.side_effect = lambda value: pending.append(copy.deepcopy(value)) is None
        queue = {'head': sources[-1], 'event_source': sources[-1], 'changes': [
            {'source': source, 'tree': 'e' * 40, 'deployables': ['cma-agent-manager']} for source in sources]}
        return queue, api, store, pending

    def test_three_pending_plus_new_source_schedule_one_build_and_keep_watermark(self):
        queue, api, store, pending = self.fixture()
        original = copy.deepcopy(pending)
        queue['changes'].reverse()  # Ancestry, not JSON ordering, determines newest.
        matrix = allocate_queue(queue, api, store, 99, 1)
        tasks = json.loads(matrix['include'][0]['tasks'])
        self.assertEqual(len(tasks), 1)
        self.assertEqual(tasks[0]['reservation']['source'], queue['head'])
        self.assertEqual(tasks[0]['reservation']['version'], '0.10.4')
        self.assertEqual(pending[:3], original)
        store.insert.assert_called_once()
        retried = allocate_queue(queue, api, store, 100, 1)
        self.assertEqual(json.loads(retried['include'][0]['tasks'])[0]['reservation']['version'], '0.10.5')
        self.assertEqual([version_tuple(r['version']) for r in pending],
                         sorted({version_tuple(r['version']) for r in pending}))

    def test_stale_queue_never_allocates_over_newer_pending_source(self):
        queue, api, store, pending = self.fixture()
        queue['changes'] = queue['changes'][:1]
        self.assertEqual(allocate_queue(queue, api, store, 99, 1), {'include': []})
        store.insert.assert_not_called()
        self.assertEqual(len(pending), 3)

    def test_active_old_owner_refuses_without_silent_loss_or_reservation_writes(self):
        queue, api, store, pending = self.fixture()
        request = api.request.side_effect
        def active(path, **kwargs):
            result = request(path, **kwargs)
            if path.startswith('actions/runs/'):
                result['status'] = 'in_progress'
            return result
        api.request.side_effect = active
        with self.assertRaisesRegex(Refusal, 'drained before supersession'):
            allocate_queue(queue, api, store, 99, 1)
        store.insert.assert_not_called()
        self.assertEqual(len(pending), 3)

    def test_shared_failed_owner_is_checked_once_per_allocation(self):
        queue, api, store, pending = self.fixture()
        for value in pending:
            value['run_id'] = 10
        allocate_queue(queue, api, store, 99, 1)
        owner_calls = [call for call in api.request.call_args_list if call.args[0].startswith('actions/runs/')]
        self.assertEqual(len(owner_calls), 1)

    def test_invalid_queue_refuses_before_any_reserve(self):
        for bad in (None, {}, {'source': 'a' * 40, 'tree': 'e' * 40, 'deployables': [{}]}):
            queue, api, store, _ = self.fixture()
            queue['changes'].append(bad)
            with self.assertRaises(Refusal):
                allocate_queue(queue, api, store, 99, 1)
            store.insert.assert_not_called()

    def test_burned_reservations_cannot_be_published_or_finalized(self):
        queue, api, store, pending = self.fixture()
        allocate_queue(queue, api, store, 99, 1)
        real = Reservations(api, 'fixture', Path.cwd(), runner=Mock())
        real.list = lambda _component=None: copy.deepcopy(pending)
        self.assertTrue(real.require_current(pending[-1]))
        for value in pending[:-1]:
            with self.assertRaisesRegex(Refusal, 'superseded and burned'):
                real.require_current(value)
        current = pending[-1]
        real.list = lambda _component=None: []
        with self.assertRaisesRegex(Refusal, 'missing'):
            real.require_current(current)
        real.runner.assert_not_called()

    def test_current_owner_can_build_but_superseded_owner_never_snapshots(self):
        identity = fixture_identity()
        reservation = dict(identity, schema='cma-version-reservation-v1')
        reservation.pop('build')
        for stale in (False, True):
            with self.subTest(stale=stale), tempfile.TemporaryDirectory() as directory, ExitStack() as stack:
                env = {'GITHUB_EVENT_NAME': 'push', 'GITHUB_RUN_ID': '100', 'GITHUB_RUN_ATTEMPT': '1',
                       'CD_COMPONENT': 'web', 'RUNNER_TEMP': directory,
                       'CD_TASKS': json.dumps([{'component': 'web', 'mode': 'publish', 'reservation': reservation}]),
                       'CD_RECEIPTS_PATH': str(Path(directory) / 'receipt.json')}
                stack.enter_context(patch.dict(os.environ, env, clear=True))
                for name in ('event_identity', 'Registry', 'GitHub'):
                    stack.enter_context(patch.object(publication, name))
                store = stack.enter_context(patch.object(publication, 'Reservations')).return_value
                store.require_current.return_value = True
                if stale:
                    store.require_current.side_effect = Refusal('Publication reservation is superseded and burned')
                snapshot = stack.enter_context(patch.object(publication, 'snapshot'))
                stack.enter_context(patch.object(publication, 'identity', return_value=(identity, {}, {})))
                stack.enter_context(patch.object(publication, 'try_reuse', return_value=None))
                build = stack.enter_context(patch.object(publication, 'build', return_value=(Path(directory), identity)))
                publish = stack.enter_context(patch.object(publication, 'publish', return_value={}))
                with redirect_stdout(io.StringIO()):
                    self.assertEqual(publication.main(), 1 if stale else 0)
                if stale:
                    snapshot.assert_not_called()
                    build.assert_not_called()
                    publish.assert_not_called()
                else:
                    build.assert_called_once()
                    self.assertTrue(publish.call_args.kwargs['reservation_check']())

    def test_supersession_during_qualification_refuses_registry_copy(self):
        for stale_at in (0, 1, None):
            with self.subTest(stale_at=stale_at), tempfile.TemporaryDirectory() as directory:
                archive, digest, registry, scanner, runner = PublicationTests().fixture(directory)
                checks = [True, True]
                if stale_at is not None:
                    checks[stale_at] = Refusal('Publication reservation is superseded and burned')
                guard = Mock(side_effect=checks)
                with patch.object(publication, 'inspect', return_value=digest):
                    def run():
                        return publication.publish(archive, fixture_identity(), Path(directory), registry,
                                                   'fixture', 'actor', scanner=scanner, runner=runner,
                                                   reservation_check=guard)
                    if stale_at is None:
                        self.assertEqual(run()['digest'], digest)
                    else:
                        with self.assertRaisesRegex(Refusal, 'superseded and burned'):
                            run()
                copies = [call for call in runner.call_args_list if call.args[0][:2] == ['skopeo', 'copy']]
                self.assertEqual(len(copies), 1 if stale_at is None else 0)
                self.assertFalse((Path(directory) / 'registry-auth.json').exists())


if __name__ == '__main__':
    unittest.main()
