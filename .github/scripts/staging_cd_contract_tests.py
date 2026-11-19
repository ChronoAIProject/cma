"""Version derivation must prove literal source and scanned image agreement."""
import copy
import json
from pathlib import Path
import unittest

from staging_cd_common import Refusal
from staging_cd_contract import (
    GROUP, RECIPE, RECIPE_LABELS, VERSION_FILE, compatibility, declared_versions, derive, from_source,
    labels, validate, verify_labels,
)
from staging_cd_receipts import annotation, canonical, IDENTITY_LABEL, parse_annotation, validate_receipt
from staging_cd_receipts_tests import fixture_receipt


def fixture_source(root):
    values = {VERSION_FILE: json.dumps({"schema": "cma-runtime-versions-v1", "runtime_contract": "3",
                                       "bootstrap": "1", "home_layout": "2"}),
              RECIPE: 'LABEL dev.chrono.cma.runtime-contract="3" '
                      'dev.chrono.cma.runtime-bootstrap="1" dev.chrono.cma.home-layout="2"'}
    for path, content in values.items():
        target = Path(root) / path
        target.parent.mkdir(parents=True, exist_ok=True)
        target.write_text(content)
    return Path(root)


def fixture_config(receipt):
    proof = receipt["compatibility"]
    found = {IDENTITY_LABEL: canonical(receipt["identity"]),
             "org.opencontainers.image.source": "https://github.com/ChronoAIProject/cma",
             "org.opencontainers.image.revision": receipt["source"],
             "org.opencontainers.image.version": receipt["identity"]["version"], **labels(proof)}
    if receipt["identity"]["component"] != "am":
        found.update({label: proof[key] for key, label in RECIPE_LABELS.items()})
    return {"architecture": "amd64", "os": "linux", "config": {"Labels": found}}


class ContractTests(unittest.TestCase):
    def test_current_source_and_every_group_component_agree(self):
        root = Path(__file__).resolve().parents[2]
        for component in GROUP:
            proof = from_source(component, "a" * 40, root)
            self.assertEqual([proof[key] for key in ("runtime_contract", "bootstrap", "home_layout")], ["3", "1", "2"])
        self.assertIsNone(from_source("web", "a" * 40, None))

    def test_duplicate_unknown_missing_and_noncanonical_declarations_refuse(self):
        source = json.dumps({"schema": "cma-runtime-versions-v1", "runtime_contract": "3",
                             "bootstrap": "1", "home_layout": "2"})
        self.assertEqual(declared_versions(source), {"runtime_contract": "3", "bootstrap": "1", "home_layout": "2"})
        bad = [source.replace('"3"', value) for value in ('3', '"03"', '"0"', '"65536"', '"3\\n"', 'null', 'true')]
        bad += [source.replace('versions-v1', 'versions-v2'), source.replace('"home_layout":', '"unknown":'),
                source.replace('"bootstrap": "1", ', ''), source[:-1] + ', "home_layout": "3"}', ' ' * 1025]
        for text in bad:
            with self.subTest(text=text), self.assertRaises(ValueError):
                declared_versions(text)

    def test_disabled_unrelated_and_test_only_rust_cannot_replace_a_declaration(self):
        comparison = ('self.image.labels.get(crate::runtime_bootstrap::RUNTIME_HOME_LAYOUT_LABEL)'
                      '.map(String::as_str) != Some("2")')
        cases = ['#[cfg(any())] impl NativePoolSpec { fn unused(&self) { if ' + comparison + ' {} } }',
                 '#![cfg(any())] impl NativePoolSpec { pub fn validate(&self) { if ' + comparison + ' {} } }',
                 '#[cfg(test)] fn helper() { if ' + comparison + ' {} }',
                 'impl Unrelated { fn helper(&self) { if ' + comparison + ' {} } }']
        for code in cases:
            # These are the review probes: none can replace an absent or invalid
            # declaration, regardless of an otherwise matching recipe.
            for absent in (None, code):
                def read(path):
                    if path == VERSION_FILE:
                        if absent is None:
                            raise FileNotFoundError(path)
                        return absent
                    if path == RECIPE:
                        return 'LABEL dev.chrono.cma.runtime-contract="3" dev.chrono.cma.runtime-bootstrap="1" dev.chrono.cma.home-layout="2"'
                    raise AssertionError('Rust source must not be inspected')
                with self.subTest(code=code, absent=absent is None), self.assertRaises((ValueError, OSError)):
                    derive('am', 'a' * 40, read)

    def test_changed_declaration_cannot_be_overridden_by_a_disabled_legacy_comparison(self):
        source = json.dumps({"schema": "cma-runtime-versions-v1", "runtime_contract": "3",
                             "bootstrap": "1", "home_layout": "3"})
        recipe = 'LABEL dev.chrono.cma.runtime-contract="3" dev.chrono.cma.runtime-bootstrap="1" dev.chrono.cma.home-layout="2"'
        files = {VERSION_FILE: source, RECIPE: recipe,
                 "api/crates/layer3-adapter/src/agent_sandbox/pool/model.rs":
                     '#[cfg(any())] impl NativePoolSpec { fn unused(&self) { if self.image.labels.get('
                     'crate::runtime_bootstrap::RUNTIME_HOME_LAYOUT_LABEL).map(String::as_str) != Some("2") {} } }'}
        with self.assertRaises(Refusal):
            derive('am', 'a' * 40, files.__getitem__)
        files[RECIPE] = recipe.replace('layout="2"', 'layout="3"')
        self.assertEqual(derive('am', 'a' * 40, files.__getitem__)["home_layout"], "3")

    def test_receipt_and_labels_bind_all_versions_to_the_exact_source(self):
        receipt = fixture_receipt(component="am")
        self.assertEqual(parse_annotation(annotation(receipt)), receipt)
        proof = receipt["compatibility"]
        verify_labels(labels(proof), proof, "am")
        runtime = dict(labels(proof), **{label: proof[key] for key, label in RECIPE_LABELS.items()})
        verify_labels(runtime, proof, "runtime")
        for key in ("runtime_contract", "bootstrap", "home_layout", "source"):
            changed = copy.deepcopy(receipt)
            changed['compatibility'][key] = "unknown"
            with self.subTest(key=key), self.assertRaises(Refusal):
                validate_receipt(changed)
        for key in runtime:
            changed = dict(runtime, **{key: "99"})
            with self.subTest(label=key), self.assertRaises(Refusal):
                verify_labels(changed, proof, "runtime")
        with self.assertRaises(Refusal):
            validate(proof, 'b' * 40)

    def test_legacy_receipts_parse_but_have_no_compatibility_authority(self):
        receipt = fixture_receipt(component="am")
        receipt.pop("compatibility")
        receipt["schema"] = "cma-published-image-v1"
        self.assertEqual(parse_annotation(annotation(receipt)), receipt)
        with self.assertRaises(Refusal):
            compatibility(receipt)
        receipt["schema"] = "cma-published-image-v2"
        with self.assertRaises(Refusal):
            validate_receipt(receipt)


if __name__ == '__main__':
    unittest.main()
