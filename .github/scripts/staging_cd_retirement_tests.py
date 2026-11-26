"""Retained release evidence never grants new retired-image publication."""
import copy
import json
from pathlib import Path
import tempfile
import unittest
from unittest.mock import Mock, patch

from ci_tracks.deployable_manifest import CURRENT, RETIRED
from deployables import components, name, registry
from staging_cd_allocate import allocate_queue
from staging_cd_allocation_snapshot import AllocationSnapshot
from staging_cd_build import recipe
import staging_cd_build_tests
from staging_cd_common import Refusal
from staging_cd_history import DevelopHistory
from staging_cd_publish import publish, validated_tasks
from image_scanner_policy import MANIFEST_SHA256
from staging_cd_receipts import annotation, parse_annotation, published_receipt
from staging_cd_receipts_tests import fixture_identity, fixture_receipt, fixture_tag
from staging_cd_reservations import (
    FIELDS, Reservations, next_reservation, pending_name, validate_reservation,
)
from staging_cd_resolve import resolve
from staging_cd_versions import plan_version


RETIRED_SOURCE = "730c1bfcb804e86531e838f84bd2133013a6f4a2"


def reservation_from(receipt):
    value = {key: value for key, value in receipt["identity"].items() if key in FIELDS}
    return dict(value, schema="cma-version-reservation-v1")


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
            reservation = reservation_from(receipt)
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

    def test_missing_retirement_ref_still_burns_version_for_next_runtime(self):
        for level, expected in (("patch", "0.10.2"), ("minor", "0.11.0"), ("major", "1.0.0")):
            with self.subTest(level=level):
                args = ("runtime", "a" * 40, "b" * 40, 42, level, 100, 1, [], [])
                reservation = next_reservation(*args)
                self.assertEqual((reservation["previous"], reservation["version"]), ("0.10.1", expected))
                self.assertEqual(plan_version("runtime", level, [], reservation["source"]).version, expected)
        for component in ("am", "eg", "web", "runtime-omarchy"):
            self.assertEqual(plan_version(component, "patch", [], RETIRED_SOURCE).version, "0.10.1")

    def test_retired_source_and_burned_version_remain_readable_but_cannot_enter_writers(self):
        receipts = (
            (fixture_receipt(component="runtime", source=RETIRED_SOURCE, previous="0.10.1", version="0.10.2"),
             "Retired release source"),
            (fixture_receipt(component="runtime", previous="0.10.0", version="0.10.1"),
             "Burned release version"),
        )
        for receipt, reason in receipts:
            with self.subTest(reason=reason):
                reservation = reservation_from(receipt)
                self.assertEqual(parse_annotation(annotation(receipt)), receipt)
                self.assertEqual(validate_reservation(reservation), reservation)
                api, runner, registry, io, check = (Mock() for _ in range(5))
                store = Reservations(api, "fixture", Path("."), runner)
                snapshot = AllocationSnapshot(api, "fixture", Path("."), io)
                snapshot.loaded = True
                env = {"CD_COMPONENT": "runtime", "GITHUB_RUN_ID": "100", "GITHUB_RUN_ATTEMPT": "1"}
                history = DevelopHistory(receipt["source"], [receipt["source"]])
                operations = (
                    lambda: store.insert(reservation),
                    lambda: snapshot.insert(reservation),
                    lambda: store.require_current(reservation, build_started=True),
                    lambda: validated_tasks([{"component": "runtime", "mode": "publish", "reservation": reservation}], env),
                    lambda: validated_tasks([{"component": "runtime", "mode": "resume", "receipt": receipt}], env),
                    lambda: publish(None, receipt["identity"], None, registry, None, None, reservation_check=check),
                    lambda: store.finalize(receipt, registry.exists),
                    lambda: store.remove_after_publication(receipt, "4" * 40),
                    lambda: resolve("runtime", "latest", [fixture_tag(receipt)], registry, history),
                )
                for operation in operations:
                    with self.assertRaisesRegex(Refusal, reason):
                        operation()
                for dependency in (api, runner, registry, io, check):
                    self.assertEqual(dependency.mock_calls, [])
                self.assertEqual(snapshot.planned, [])
        with self.assertRaisesRegex(Refusal, "Retired release source"):
            next_reservation("runtime", RETIRED_SOURCE, "b" * 40, 42, "patch", 100, 1, [], [])
        with self.assertRaisesRegex(Refusal, "Retired release source"):
            plan_version("runtime", "patch", [], RETIRED_SOURCE)

    def test_historical_runtime_selection_cannot_fall_back_or_block_other_components(self):
        older, successor = "a" * 40, "f" * 40
        rows = [
            {"source": older, "tree": "b" * 40, "deployables": ["cma-runtime"]},
            {"source": RETIRED_SOURCE, "tree": "c" * 40, "deployables": ["cma-runtime", "cma-web"]},
        ]
        retired = reservation_from(fixture_receipt(component="runtime", source=RETIRED_SOURCE, tree="c" * 40,
                                                   previous="0.10.0", version="0.10.1"))
        for has_ref in (True, False):
            with self.subTest(has_ref=has_ref):
                api, store = Mock(), Mock()
                api.tags.return_value = []
                store.list.return_value = [retired] if has_ref else []
                api.history.return_value = DevelopHistory(successor, [successor, RETIRED_SOURCE, older])
                queue = {"head": successor, "event_source": successor, "changes": rows}
                with patch("staging_cd_allocate.merged_pr", return_value={"number": 42, "labels": []}) as merged:
                    matrix = allocate_queue(queue, api, store, 1000, 1)["include"]
                    self.assertEqual([job["component"] for job in matrix], ["web"])
                    merged.assert_called_once_with(RETIRED_SOURCE, api.request)
                    self.assertEqual(store.insert.call_args.args[0]["source"], RETIRED_SOURCE)
                    queue["changes"] = rows + [
                        {"source": successor, "tree": "d" * 40, "deployables": ["cma-runtime"]},
                    ]
                    matrix = allocate_queue(queue, api, store, 1000, 1)["include"]
                    runtime = next(job for job in matrix if job["component"] == "runtime")
                    value = json.loads(runtime["tasks"])[0]["reservation"]
                    self.assertEqual((value["source"], value["version"]), (successor, "0.10.2"))
                api.request.assert_not_called()

    def test_original_burned_reservation_is_preserved_by_pruning(self):
        retired = reservation_from(fixture_receipt(component="runtime", source=RETIRED_SOURCE,
                                                   previous="0.10.0", version="0.10.1"))
        current = fixture_receipt(component="runtime", source="f" * 40, run_id=101,
                                  previous="0.10.1", version="0.10.2")
        api, runner = Mock(), Mock()
        api.history.return_value = DevelopHistory(current["source"], [current["source"], RETIRED_SOURCE])
        api.tags.return_value = [fixture_tag(current)]
        store = Reservations(api, "fixture", Path("."), runner)
        ref = "refs/tags/" + pending_name(retired)
        store.advertised = Mock(return_value=({ref: "d" * 40}, {}))
        store.inventory = Mock(return_value=[(retired, "d" * 40)])
        self.assertEqual(store.prune_burned(current), 0)
        runner.assert_not_called()


if __name__ == '__main__':
    unittest.main()
