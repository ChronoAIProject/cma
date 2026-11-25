"""Only exact qualified runtime releases can advance an environment pointer."""

import unittest
from unittest.mock import Mock

from runtime_pointer_publication import advance
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
                registry, copy = Mock(), Mock()
                registry.manifest.return_value = (candidate["digest"], {})
                registry.runtime_pointer.side_effect = [previous["digest"], previous["digest"], candidate["digest"]]
                history = DevelopHistory(CANDIDATE, [CANDIDATE, OLDER])
                fence = Mock()
                self.assertTrue(advance(component, candidate, [fixture_tag(previous), fixture_tag(candidate)],
                                        history, registry, copy, environment="staging", fence=fence))
                fence.assert_called_once_with({component: candidate})
                copy.assert_called_once_with(candidate["repository"], candidate["digest"], "staging-current")
                registry.runtime_pointer.assert_called_with(candidate["repository"], "staging", settle=True)

    def test_production_first_publication_and_replay(self):
        candidate = release("runtime", CANDIDATE, "2")
        registry, copy = Mock(), Mock()
        registry.manifest.return_value = (candidate["digest"], {})
        registry.runtime_pointer.side_effect = [None, None, candidate["digest"], candidate["digest"]]
        history = DevelopHistory(CANDIDATE, [CANDIDATE])
        args = ("runtime", candidate, [fixture_tag(candidate)], history, registry, copy)
        fence = Mock()
        self.assertTrue(advance(*args, environment="production", fence=fence))
        self.assertFalse(advance(*args, environment="production", fence=fence))
        copy.assert_called_once_with(candidate["repository"], candidate["digest"], "production-current")
        fence.assert_called_once()

    def test_unknown_and_later_predecessors_cannot_be_overwritten(self):
        candidate = release("runtime", CANDIDATE, "2")
        newer = release("runtime", NEWER, "3")
        registry, copy = Mock(), Mock()
        registry.manifest.return_value = (candidate["digest"], {})
        registry.runtime_pointer.return_value = newer["digest"]
        history = DevelopHistory(NEWER, [NEWER, CANDIDATE])
        fence = Mock()
        for extra in ([], [fixture_tag(newer)]):
            with self.subTest(extra=bool(extra)), self.assertRaises(Refusal):
                advance("runtime", candidate, [fixture_tag(candidate), *extra], history, registry, copy,
                        environment="staging", fence=fence)
            copy.assert_not_called()
            fence.assert_not_called()

    def test_invalid_candidate_and_wrong_readback_refuse(self):
        candidate = release("runtime", CANDIDATE, "2")
        registry, copy = Mock(), Mock()
        registry.manifest.return_value = (candidate["digest"], {})
        history = DevelopHistory(CANDIDATE, [CANDIDATE])
        args = ("runtime", candidate, [fixture_tag(candidate)], history, registry, copy)
        fence = Mock()
        with self.assertRaisesRegex(Refusal, "environment is invalid"):
            advance(*args, environment="sandbox", fence=fence)
        with self.assertRaisesRegex(Refusal, "no matching publication"):
            advance("runtime", candidate, [], history, registry, copy, environment="staging", fence=fence)
        with self.assertRaises(Refusal):
            advance("runtime", dict(candidate, runtime_smoke=None), [fixture_tag(candidate)], history,
                    registry, copy, environment="staging", fence=fence)
        copy.assert_not_called()
        registry.runtime_pointer.side_effect = [None, None, "sha256:" + "3" * 64]
        with self.assertRaisesRegex(Refusal, "selected digest"):
            advance(*args, environment="staging", fence=fence)
        copy.assert_called_once()

    def test_late_supersession_and_changed_pointer_refuse_before_copy(self):
        candidate = release("runtime", CANDIDATE, "2")
        registry, copy = Mock(), Mock()
        registry.manifest.return_value = (candidate["digest"], {})
        history = DevelopHistory(CANDIDATE, [CANDIDATE])
        args = ("runtime", candidate, [fixture_tag(candidate)], history, registry, copy)
        registry.runtime_pointer.side_effect = [None, None, "sha256:" + "3" * 64]
        with self.assertRaisesRegex(Refusal, "superseded"):
            advance(*args, environment="staging", fence=Mock(side_effect=Refusal("superseded")))
        with self.assertRaisesRegex(Refusal, "changed before publication"):
            advance(*args, environment="staging", fence=Mock())
        copy.assert_not_called()

    def test_uncertain_copy_uses_the_observed_tag_without_repeating_the_write(self):
        candidate = release("runtime", CANDIDATE, "2")
        registry = Mock()
        registry.manifest.return_value = (candidate["digest"], {})
        history = DevelopHistory(CANDIDATE, [CANDIDATE])
        copy = Mock(side_effect=TimeoutError("copy acknowledgement lost"))
        args = ("runtime", candidate, [fixture_tag(candidate)], history, registry, copy)
        registry.runtime_pointer.side_effect = [None, None, candidate["digest"]]
        self.assertTrue(advance(*args, environment="staging", fence=Mock()))
        copy.assert_called_once()
        registry.runtime_pointer.side_effect = [None, None, None]
        copy.reset_mock()
        with self.assertRaises(TimeoutError):
            advance(*args, environment="staging", fence=Mock())
        copy.assert_called_once()
