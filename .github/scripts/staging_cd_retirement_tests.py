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
                published_receipt(receipt['identity'], receipt['digest'], receipt['archive'], receipt['scan']['counts'], bundle=MANIFEST_SHA256)
            with self.assertRaises(Refusal):
                next_reservation(component, 'a' * 40, 'b' * 40, 42, 'patch', 1, 1, [], [])
            with self.assertRaises(Refusal):
                plan_version(component, 'patch', [fixture_tag(receipt)], 'a' * 40)
            api, runner, verify = Mock(), Mock(), Mock()
            store = Reservations(api, 'fixture', Path('.'), runner)
            for operation in (lambda: store.insert(reservation), lambda: store.finalize(receipt, verify),
                              lambda: store.remove_after_publication(receipt)):
                with self.assertRaises(Refusal):
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


if __name__ == '__main__':
    unittest.main()
