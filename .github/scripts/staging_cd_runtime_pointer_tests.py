"""Only exact qualified runtime releases can advance an environment pointer."""

import unittest
from unittest.mock import Mock

from runtime_pointer_publication import advance, await_active, promote
from staging_cd_common import Refusal
from staging_cd_history import DevelopHistory
from staging_cd_receipts_tests import fixture_runtime_receipt, fixture_tag


OLDER, CANDIDATE, NEWER = "a" * 40, "b" * 40, "c" * 40


def release(component, source, digest_digit):
    receipt = fixture_runtime_receipt(component, source=source)
    receipt["digest"] = receipt["runtime_smoke"]["digest"] = "sha256:" + digest_digit * 64
    return receipt


class PointerPublicationTests(unittest.TestCase):
    def test_both_staging_lanes_use_their_qualified_digest(self):
        for component in ("runtime", "runtime-omarchy"):
            with self.subTest(component=component):
                previous = release(component, OLDER, "1")
                candidate = release(component, CANDIDATE, "2")
                registry = Mock()
                registry.manifest.return_value = (candidate["digest"], {})
                registry.runtime_pointer.side_effect = [previous["digest"], previous["digest"], candidate["digest"]]
                history = DevelopHistory(CANDIDATE, [CANDIDATE, OLDER])
                fence = Mock()
                self.assertTrue(advance(component, candidate, [fixture_tag(previous), fixture_tag(candidate)],
                                        history, registry, environment="staging", fence=fence))
                fence.assert_called_once_with({component: candidate})
                registry.prepare_runtime_pointer.assert_called_once_with(candidate["repository"], "staging", candidate["digest"])
                registry.write_runtime_pointer.assert_called_once_with(registry.prepare_runtime_pointer.return_value,
                                                                       candidate["digest"])
                registry.runtime_pointer.assert_called_with(candidate["repository"], "staging", settle=True)

    def test_production_first_publication_and_replay(self):
        candidate = release("runtime", CANDIDATE, "2")
        registry = Mock()
        registry.manifest.return_value = (candidate["digest"], {})
        registry.runtime_pointer.side_effect = [None, None, candidate["digest"], candidate["digest"]]
        history = DevelopHistory(CANDIDATE, [CANDIDATE])
        args = ("runtime", candidate, [fixture_tag(candidate)], history, registry)
        fence = Mock()
        self.assertTrue(advance(*args, environment="production", fence=fence))
        self.assertFalse(advance(*args, environment="production", fence=fence))
        registry.prepare_runtime_pointer.assert_called_once_with(candidate["repository"], "production", candidate["digest"])
        registry.write_runtime_pointer.assert_called_once_with(registry.prepare_runtime_pointer.return_value,
                                                               candidate["digest"])
        fence.assert_called_once()

    def test_unknown_and_later_predecessors_cannot_be_overwritten(self):
        candidate = release("runtime", CANDIDATE, "2")
        newer = release("runtime", NEWER, "3")
        registry = Mock()
        registry.manifest.return_value = (candidate["digest"], {})
        registry.runtime_pointer.return_value = newer["digest"]
        history = DevelopHistory(NEWER, [NEWER, CANDIDATE])
        fence = Mock()
        for extra in ([], [fixture_tag(newer)]):
            with self.subTest(extra=bool(extra)), self.assertRaises(Refusal):
                advance("runtime", candidate, [fixture_tag(candidate), *extra], history, registry,
                        environment="staging", fence=fence)
            registry.write_runtime_pointer.assert_not_called()
            fence.assert_not_called()

    def test_invalid_candidate_and_wrong_readback_refuse(self):
        candidate = release("runtime", CANDIDATE, "2")
        registry = Mock()
        registry.manifest.return_value = (candidate["digest"], {})
        history = DevelopHistory(CANDIDATE, [CANDIDATE])
        args = ("runtime", candidate, [fixture_tag(candidate)], history, registry)
        fence = Mock()
        with self.assertRaisesRegex(Refusal, "environment is invalid"):
            advance(*args, environment="sandbox", fence=fence)
        with self.assertRaisesRegex(Refusal, "no matching publication"):
            advance("runtime", candidate, [], history, registry, environment="staging", fence=fence)
        with self.assertRaises(Refusal):
            advance("runtime", dict(candidate, runtime_smoke=None), [fixture_tag(candidate)], history,
                    registry, environment="staging", fence=fence)
        registry.write_runtime_pointer.assert_not_called()
        registry.runtime_pointer.side_effect = [None, None, "sha256:" + "3" * 64]
        with self.assertRaisesRegex(Refusal, "selected digest"):
            advance(*args, environment="staging", fence=fence)
        registry.write_runtime_pointer.assert_called_once()

    def test_late_supersession_and_changed_pointer_refuse_before_copy(self):
        candidate = release("runtime", CANDIDATE, "2")
        registry = Mock()
        registry.manifest.return_value = (candidate["digest"], {})
        history = DevelopHistory(CANDIDATE, [CANDIDATE])
        args = ("runtime", candidate, [fixture_tag(candidate)], history, registry)
        registry.runtime_pointer.side_effect = [None, None, None, "sha256:" + "3" * 64]
        with self.assertRaisesRegex(Refusal, "superseded"):
            advance(*args, environment="staging", fence=Mock(side_effect=Refusal("superseded")))
        with self.assertRaisesRegex(Refusal, "changed before publication"):
            advance(*args, environment="staging", fence=Mock())
        registry.write_runtime_pointer.assert_not_called()

    def test_blackout_entered_during_registry_preparation_refuses_before_put(self):
        candidate = release("runtime", CANDIDATE, "2")
        registry = Mock()
        registry.manifest.return_value = (candidate["digest"], {})
        registry.runtime_pointer.return_value = None
        in_blackout = False

        def prepare(*_args):
            nonlocal in_blackout
            in_blackout = True

        def fence(_candidates):
            if in_blackout:
                raise Refusal("production blackout")

        registry.prepare_runtime_pointer.side_effect = prepare
        with self.assertRaisesRegex(Refusal, "production blackout"):
            advance("runtime", candidate, [fixture_tag(candidate)], DevelopHistory(CANDIDATE, [CANDIDATE]),
                    registry, environment="production", fence=fence)
        registry.write_runtime_pointer.assert_not_called()

    def test_uncertain_copy_uses_the_observed_tag_without_repeating_the_write(self):
        candidate = release("runtime", CANDIDATE, "2")
        registry = Mock()
        registry.manifest.return_value = (candidate["digest"], {})
        history = DevelopHistory(CANDIDATE, [CANDIDATE])
        registry.write_runtime_pointer.side_effect = TimeoutError("copy acknowledgement lost")
        args = ("runtime", candidate, [fixture_tag(candidate)], history, registry)
        registry.runtime_pointer.side_effect = [None, None, candidate["digest"]]
        self.assertTrue(advance(*args, environment="staging", fence=Mock()))
        registry.write_runtime_pointer.assert_called_once()
        registry.runtime_pointer.side_effect = [None, None, None]
        registry.write_runtime_pointer.reset_mock()
        with self.assertRaises(TimeoutError):
            advance(*args, environment="staging", fence=Mock())
        registry.write_runtime_pointer.assert_called_once()

class ActiveRuntimeTests(unittest.TestCase):
    def test_both_durable_lanes_must_be_active_on_the_ready_manager(self):
        agent = release("runtime", CANDIDATE, "2")
        machine = release("runtime-omarchy", CANDIDATE, "3")
        accepted = {"agent": {"digest": agent["digest"], "revision": 2},
                    "machine": {"digest": machine["digest"], "revision": 3}}
        pending = dict(accepted, machine={"digest": machine["digest"], "revision": 0})
        read = Mock(side_effect=[{"status": "not-ready", "runtime_release": accepted},
                                  {"status": "ready", "runtime_release": pending},
                                  {"status": "ready", "runtime_release": accepted}])
        current = [0]
        await_active({"runtime": agent, "runtime-omarchy": machine}, read, deadline=16,
                     clock=lambda: current[0], sleep=lambda delay: current.__setitem__(0, current[0] + delay))
        self.assertEqual([call.args for call in read.call_args_list], [(16,), (11,), (6,)])

    def test_old_active_runtime_exhausts_the_budget(self):
        agent = release("runtime", CANDIDATE, "2")
        old = release("runtime", OLDER, "1")
        read = Mock(return_value={"status": "ready", "runtime_release": {
            "agent": {"digest": old["digest"], "revision": 1},
        }})
        current = [0]
        with self.assertRaisesRegex(Refusal, "did not promote"):
            await_active({"runtime": agent}, read, deadline=8, clock=lambda: current[0],
                         sleep=lambda delay: current.__setitem__(0, current[0] + delay))
        self.assertEqual([call.args for call in read.call_args_list], [(8,), (3,)])

    def test_pointer_write_only_completes_after_agent_manager_activates_candidate(self):
        agent = release("runtime", CANDIDATE, "2")
        old = release("runtime", OLDER, "1")
        registry = Mock()
        registry.manifest.return_value = (agent["digest"], {})
        registry.runtime_pointer.side_effect = [old["digest"], old["digest"], agent["digest"]]
        read = Mock(side_effect=[
            {"status": "ready", "runtime_release": {"agent": {"digest": old["digest"], "revision": 1}}},
            {"status": "ready", "runtime_release": {"agent": {"digest": agent["digest"], "revision": 2}}},
        ])
        current = [0]
        promote({"runtime": agent}, {"runtime": [fixture_tag(old), fixture_tag(agent)]},
                DevelopHistory(CANDIDATE, [CANDIDATE, OLDER]), registry, read,
                environment="staging", fence=Mock(), deadline=8, clock=lambda: current[0],
                sleep=lambda delay: current.__setitem__(0, current[0] + delay))
        registry.prepare_runtime_pointer.assert_called_once_with(agent["repository"], "staging", agent["digest"])
        registry.write_runtime_pointer.assert_called_once_with(registry.prepare_runtime_pointer.return_value,
                                                               agent["digest"])
        self.assertEqual([call.args for call in read.call_args_list], [(8,), (3,)])

    def test_omarchy_is_refused_before_any_production_pointer_write(self):
        machine = release("runtime-omarchy", CANDIDATE, "3")
        registry, read = Mock(), Mock()
        with self.assertRaisesRegex(Refusal, "restricted to staging"):
            promote({"runtime-omarchy": machine}, {}, DevelopHistory(CANDIDATE, [CANDIDATE]),
                    registry, read, environment="production", fence=Mock(), deadline=8)
        self.assertEqual(registry.mock_calls, [])
        read.assert_not_called()
