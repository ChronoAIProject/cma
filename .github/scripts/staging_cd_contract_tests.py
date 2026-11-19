"""Version derivation must prove literal source and scanned image agreement."""
import copy
from pathlib import Path
import unittest

from staging_cd_common import Refusal
from staging_cd_contract import (
    GROUP, POOL_SOURCE, RECIPE, RECIPE_LABELS, SOURCES, compatibility, derive, from_source,
    home_layout, labels, literal_constant, validate, verify_labels,
)
from staging_cd_receipts import annotation, canonical, IDENTITY_LABEL, parse_annotation, validate_receipt
from staging_cd_receipts_tests import fixture_receipt


def fixture_source(root):
    values = {SOURCES[0]: 'pub const RUNTIME_CONTRACT_VERSION: &str = "3";',
              SOURCES[1]: 'pub const RUNTIME_BOOTSTRAP_VERSION: u16 = 1;',
              POOL_SOURCE: 'impl Pool { fn validate(&self) { if self.image.labels.get('
              'crate::runtime_bootstrap::RUNTIME_HOME_LAYOUT_LABEL).map(String::as_str) != Some("2") {} } }',
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

    def test_comments_macros_conditions_and_expressions_cannot_supply_versions(self):
        good = 'pub const RUNTIME_CONTRACT_VERSION: &str = "3";'
        self.assertEqual(literal_constant('// comment\n' + good, "RUNTIME_CONTRACT_VERSION", "string_literal"), "3")
        self.assertEqual(literal_constant('/* comment */\n' + good, "RUNTIME_CONTRACT_VERSION", "string_literal"), "3")
        cases = ['/*' + good + '*/', '//' + good, 'macro_rules! m { () => {' + good + '} }',
                 '#[cfg(feature="alternate")]\n' + good, '#![cfg(feature="alternate")]\n' + good,
                 good + good, 'mod nested {' + good + '}', good.replace('"3"', 'concat!("3")')]
        for text in cases:
            with self.subTest(text=text), self.assertRaises(Refusal):
                literal_constant(text, "RUNTIME_CONTRACT_VERSION", "string_literal")

    def test_home_layout_must_be_a_real_comparison_and_recipe_must_agree(self):
        code = ('impl Pool { fn validate(&self) { if self.image.labels.get('
                'crate::runtime_bootstrap::RUNTIME_HOME_LAYOUT_LABEL).map(String::as_str) != Some("2") {} } }')
        self.assertEqual(home_layout(code), "2")
        for text in ('/*' + code + '*/', code.replace('Some("2")', 'expected()'), code + code):
            with self.subTest(text=text), self.assertRaises(Refusal):
                home_layout(text)
        sources = {SOURCES[0]: 'pub const RUNTIME_CONTRACT_VERSION: &str = "3";',
                   SOURCES[1]: 'pub const RUNTIME_BOOTSTRAP_VERSION: u16 = 1;', POOL_SOURCE: code,
                   RECIPE: 'LABEL dev.chrono.cma.runtime-contract="4" '
                           'dev.chrono.cma.runtime-bootstrap="1" dev.chrono.cma.home-layout="2"'}
        with self.assertRaises(Refusal):
            derive('am', 'a' * 40, sources.__getitem__)

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
