"""Retained release evidence never grants new retired-image publication."""
import copy
import json
from pathlib import Path
import tempfile
import unittest
from unittest.mock import Mock

from ci_tracks.deployable_manifest import CURRENT, RETIRED
from deployables import components, name, registry
from staging_cd_build import recipe
import staging_cd_build_tests
from staging_cd_common import Refusal
from image_scanner_policy import MANIFEST_SHA256
from staging_cd_receipts import annotation, parse_annotation, published_receipt
from staging_cd_publish import publish, validated_tasks
from staging_cd_history import DevelopHistory
from staging_cd_allocation_snapshot import AllocationSnapshot
from staging_cd_receipts_tests import fixture_identity, fixture_receipt, fixture_tag
from staging_cd_reservations import (
    FIELDS, Reservations, next_reservation, pending_name, validate_reservation,
)
from staging_cd_versions import plan_version


class RetirementTests(unittest.TestCase):
    def test_current_registry_has_exactly_five_publication_identities(self):
        self.assertEqual(set(registry()), set(CURRENT))
        self.assertEqual(set(components().values()), set(CURRENT))
        for component in RETIRED.values():
            with self.assertRaises(ValueError):
                name(component)

    def test_historical_source_can_build_active_images_but_not_retired_images(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            staging_cd_build_tests.BuildTests().source(root)
            path = root / '.github/scripts/deployables.json'
            data = json.loads(path.read_text())
            for image, component in RETIRED.items():
                data[image] = dict(copy.deepcopy(data['cma-web']), component=component)
            path.write_text(json.dumps(data))
            self.assertEqual(recipe(root, 'web')['component'], 'web')
            for component in RETIRED.values():
                with self.assertRaises(Refusal):
                    recipe(root, component)

    def test_retained_receipts_roundtrip_but_cannot_publish_or_modify_tags(self):
        for image, component in RETIRED.items():
            receipt = fixture_receipt()
            receipt['identity']['component'] = component
            receipt.update(tag=image + '/v0.10.1', repository='ghcr.io/chronoaiproject/' + image)
            self.assertEqual(parse_annotation(annotation(receipt)), receipt)
            reservation = {key: value for key, value in receipt['identity'].items() if key in FIELDS}
            reservation['schema'] = 'cma-version-reservation-v1'
            self.assertEqual(validate_reservation(reservation), reservation)
            self.assertEqual(pending_name(reservation), image + '/pending/v0.10.1')
            with self.assertRaises(Refusal):
                published_receipt(receipt['identity'], receipt['digest'], receipt['archive'], receipt['scan']['counts'], scanner_identity=("1.0.4", MANIFEST_SHA256))
            with self.assertRaises(Refusal):
                next_reservation(component, 'a' * 40, 'b' * 40, 42, 'patch', 1, 1, [], [])
            with self.assertRaises(Refusal):
                plan_version(component, 'patch', [fixture_tag(receipt)], 'a' * 40)
            api, runner, verify = Mock(), Mock(), Mock()
            store = Reservations(api, 'fixture', Path('.'), runner)
            for operation, reason in (
                (lambda: store.insert(reservation), "Retired images cannot reserve versions"),
                (lambda: store.finalize(receipt, verify), "Retired image tags must remain unchanged"),
                (lambda: store.remove_after_publication(receipt, "a" * 40), "Retired image tags must remain unchanged"),
            ):
                with self.subTest(component=component, reason=reason), self.assertRaisesRegex(Refusal, reason):
                    operation()
            api.assert_not_called()
            self.assertEqual(api.mock_calls, [])
            runner.assert_not_called()
            verify.assert_not_called()

    def test_retired_runtime_source_is_readable_but_never_enters_release_writers(self):
        source = "730c1bfcb804e86531e838f84bd2133013a6f4a2"
        receipt = fixture_receipt(component="runtime", source=source)
        self.assert_release_writers_refuse(receipt, "Retired source cannot be published")
        with self.assertRaisesRegex(Refusal, "Retired source cannot be published"):
            next_reservation("runtime", source, "b" * 40, 42, "patch", 1, 1, [], [])
        with self.assertRaisesRegex(Refusal, "Retired source cannot be published"):
            plan_version("runtime", "patch", [], source)

    def test_burned_runtime_version_refuses_a_different_source(self):
        receipt = fixture_receipt(component="runtime", source="e" * 40)
        self.assert_release_writers_refuse(receipt, "Burned release version cannot be reused")

    def assert_release_writers_refuse(self, receipt, reason):
        identity = receipt["identity"]
        reservation = {key: value for key, value in identity.items() if key in FIELDS}
        reservation["schema"] = "cma-version-reservation-v1"
        self.assertEqual(parse_annotation(annotation(receipt)), receipt)
        self.assertEqual(validate_reservation(reservation), reservation)

        api, runner, verify = Mock(), Mock(), Mock()
        publisher = Reservations(api, "fixture", Path("."), runner)
        allocation = AllocationSnapshot(api, "fixture", Path("."), Mock())
        allocation.loaded = True
        env = {"CD_COMPONENT": "runtime", "GITHUB_RUN_ID": str(identity["run_id"]),
               "GITHUB_RUN_ATTEMPT": str(identity["run_attempt"])}
        registry, reservation_check = Mock(), Mock(return_value=True)
        operations = {
            "legacy insert": lambda: publisher.insert(reservation),
            "atomic insert": lambda: allocation.insert(reservation),
            "publish task": lambda: validated_tasks(
                [{"component": "runtime", "mode": "publish", "reservation": reservation}], env),
            "resume task": lambda: validated_tasks(
                [{"component": "runtime", "mode": "resume", "receipt": receipt}], env),
            "pre-build authority": lambda: publisher.require_current(reservation, build_started=False),
            "started-build authority": lambda: publisher.require_current(reservation, build_started=True),
            "publish": lambda: publish(None, identity, Path("."), registry, "fixture", "actor",
                                       reservation_check=reservation_check),
            "finalize": lambda: publisher.finalize(receipt, verify),
        }
        for action, operation in operations.items():
            with self.subTest(action=action), self.assertRaisesRegex(Refusal, reason):
                operation()
        self.assertEqual(registry.mock_calls, [])
        reservation_check.assert_not_called()
        self.assertEqual(allocation.planned, [])
        self.assertEqual(api.mock_calls, [])
        runner.assert_not_called()
        verify.assert_not_called()

    def test_retirement_preserves_other_components_at_the_same_source(self):
        source = "730c1bfcb804e86531e838f84bd2133013a6f4a2"
        self.assertEqual(next_reservation("web", source, "a" * 40, 42, "patch", 1, 1, [], [])["source"],
                         source)
        self.assertEqual(plan_version("runtime", "patch", [], "b" * 40).version, "0.10.2")
        replacement = next_reservation("runtime", "b" * 40, "c" * 40, 42, "patch", 1, 1, [], [])
        self.assertEqual((replacement["previous"], replacement["version"]), ("0.10.1", "0.10.2"))

    def test_a_later_final_does_not_prune_the_original_runtime_reservation(self):
        source = "730c1bfcb804e86531e838f84bd2133013a6f4a2"
        retired = {key: value for key, value in fixture_identity(component="runtime", source=source).items()
                   if key in FIELDS}
        retired["schema"] = "cma-version-reservation-v1"
        current = fixture_receipt(component="runtime", source="c" * 40, previous="0.10.1", version="0.10.2")
        api, runner = Mock(), Mock()
        api.history.return_value = DevelopHistory(current["source"], [current["source"], source])
        publisher = Reservations(api, "fixture", Path("."), runner)
        publisher.settled["runtime"] = [fixture_tag(current)]
        pending_ref = "refs/tags/" + pending_name(retired)
        publisher.advertised = Mock(return_value=({pending_ref: "d" * 40}, {}))
        publisher.inventory = Mock(return_value=[(retired, "d" * 40)])

        self.assertEqual(publisher.prune_burned(current), 0)
        runner.assert_not_called()

    def test_active_version_history_and_burned_reservations_survive_retirement(self):
        receipt = fixture_receipt()
        retired = dict(fixture_identity(component='runtime-desktop'), schema='cma-version-reservation-v1')
        del retired['build']
        pending = next_reservation('web', 'f' * 40, 'b' * 40, 43, 'patch', 2, 1,
                                   [fixture_tag(receipt)], [retired])
        self.assertEqual(pending['version'], '0.10.2')
        following = next_reservation('web', 'f' * 40, 'b' * 40, 43, 'patch', 3, 1,
                                     [fixture_tag(receipt)], [retired, pending])
        self.assertEqual(following['version'], '0.10.3')
        self.assertEqual(plan_version('web', 'major', [fixture_tag(receipt)], receipt['source']).receipt, receipt)


if __name__ == '__main__':
    unittest.main()
