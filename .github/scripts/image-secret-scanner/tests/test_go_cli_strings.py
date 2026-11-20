"""Recheck exact adjacent Go strings through the staging publication gate."""

import struct
import unittest

from image_secrets import go_cli_strings
from image_secrets.common import digest_bytes
from image_secrets.gate import GO_CLI_ADVISORY
from tests import test_derived_payload as derived
from tests import test_package_policy as packages

RULE = "gitleaks.github-oauth"
FIRST = b"gho" + b"_"
SECOND = b"Ab01Cd23Ef45Gh67Ij89Kl02Mn34Op56Qr78"
ADDRESS = 0x400000
LITERAL = 0x180
INSTRUCTION = 0x320
HEADER = 0x440


def elf(*, wrong_length=False, extra=b"", stack=False, stack_register=0, extra_mapping=None):
    data = bytearray(0x580)
    data[:7] = b"\x7fELF\x02\x01\x01"
    struct.pack_into("<HHIQQQIHHHHHH", data, 16, 2, 62, 1, 0x400320, 64, 0, 0,
                     64, 56, 3 + int(extra_mapping is not None), 0, 0, 0)
    for index, (flags, offset, size) in enumerate(((4, 0, 0x300), (5, 0x300, 0x100), (6, 0x400, 0x180))):
        struct.pack_into("<IIQQQQQQ", data, 64 + 56 * index, 1, flags, offset, ADDRESS + offset,
                         ADDRESS + offset, size, size, 0x1000)
    if extra_mapping is not None:
        offset, virtual, file_size, memory_size = extra_mapping
        struct.pack_into("<IIQQQQQQ", data, 64 + 56 * 3, 1, 6, offset, virtual, virtual, file_size, memory_size, 1)
    data[LITERAL:LITERAL + len(FIRST + SECOND)] = FIRST + SECOND
    data[0x220:0x220 + len(extra)] = extra
    operand = LITERAL + len(FIRST) - (INSTRUCTION + 7)
    data[INSTRUCTION:INSTRUCTION + 7] = b"\x48\x8d" + bytes((0x05 + 8 * stack_register,)) + struct.pack("<i", operand)
    size = struct.pack("<I", len(SECOND) - int(wrong_length))
    suffix = (b"\x48\x89" + bytes((0x44 + 8 * stack_register,)) + b"\x24\x18\x48\xc7\x44\x24\x20"
              if stack else b"\xbb")
    data[INSTRUCTION + 7:INSTRUCTION + 11 + len(suffix)] = suffix + size
    struct.pack_into("<QQ", data, HEADER, ADDRESS + LITERAL, len(FIRST))
    return bytes(data)


class GoCliStringsTests(unittest.TestCase):
    setUp = packages.PackageTests.setUp
    tearDown = packages.PackageTests.tearDown
    scan = packages.PackageTests.scan
    manifest = packages.PackageTests.manifest
    detector_scan = packages.PackageTests.detector_scan
    fixture = derived.DerivedPayloadTests.fixture
    binding = derived.DerivedPayloadTests.binding
    publish = derived.DerivedPayloadTests.publish

    def evidence(self, name, *, wrong_length=False, extra=b"", stack=False, stack_register=0, extra_mapping=None):
        data = elf(wrong_length=wrong_length, extra=extra, stack=stack,
                   stack_register=stack_register, extra_mapping=extra_mapping)
        source = ("cli-fixture/go.sum", b"fixture source input")
        archive, manifest, raw, _ = self.fixture(name, go_cli_strings.PATH, source, data)
        manifest["packages"][0].update(name="github-cli", authority_type="pinned-upstream")
        [row] = manifest["entries"]
        row["path"] = go_cli_strings.PATH
        candidate = FIRST + SECOND
        [finding] = [f for f in raw["blocking_findings"] if f["rule"] == RULE
                     and f["candidate_sha256"] == digest_bytes(candidate)]
        row["matches"] = [{"rule": finding["rule"], "line": finding.get("line"), "explanation": GO_CLI_ADVISORY,
                           "candidate_sha256": digest_bytes(candidate), "proof_sha256": "4" * 64}]
        proof = self.binding(row, raw, finding, "go-cli-strings", data)

        def span(start, length):
            return {"offset": start, "size": length, "sha256": digest_bytes(data[start:start + length])}

        public = {"path": "fixture/source.go", "line": 12, "sha256": "5" * 64, "proof_sha256": "6" * 64}
        references = [{"kind": "word-pair", "offset": HEADER, "sha256": digest_bytes(data[HEADER:HEADER + 16])},
                      {"kind": "instruction-pair", "address": span(INSTRUCTION, 7), "length": span(INSTRUCTION + 7, 5)}]
        if stack:
            references[1] = {"kind": "stack-pair", "code": span(INSTRUCTION, 21)}
        parts = [{"literal": span(offset, len(value)), "contribution_sha256": digest_bytes(value),
                  "source": public, "reference": ref}
                 for offset, value, ref in zip((LITERAL, LITERAL + len(FIRST)), (FIRST, SECOND), references)]
        proof["strings"] = [{"finding_id": finding["finding_id"], "rule": RULE,
                             "candidate": span(LITERAL, len(candidate)), "parts": parts}]
        return archive, manifest, raw, finding["finding_id"]

    def test_verified_pointer_and_instruction_pairs_preserve_raw_findings(self):
        archive, manifest, raw, finding = self.evidence("valid")
        accepted = self.publish(archive, manifest, "valid-go")
        self.assertEqual(accepted["findings"], raw["findings"])
        [receipt] = accepted["package_evidence"]["entries"]
        self.assertEqual(receipt["finding_id"], finding)
        self.assertEqual(receipt["classification"], GO_CLI_ADVISORY)

    def test_different_length_refuses_the_entire_candidate(self):
        for name, stack in (("register", False), ("stack", True)):
            with self.subTest(reference=name):
                archive, manifest, raw, _ = self.evidence(f"wrong-{name}", wrong_length=True, stack=stack)
                refused = self.publish(archive, manifest, f"wrong-go-{name}-length", blocks=True)
                self.assertEqual(refused["package_evidence"]["entries"], [])
                self.assertEqual(refused["blocking_findings"], raw["blocking_findings"])

    def test_stack_pair_reproduces_the_pointer_and_count(self):
        archive, manifest, raw, _ = self.evidence("stack", stack=True)
        result = self.publish(archive, manifest, "accepted-stack")
        self.assertEqual(result["findings"], raw["findings"])
        self.assertEqual(len(result["package_evidence"]["entries"]), 1)

    def test_stack_pointer_cannot_hold_the_literal_address(self):
        archive, manifest, raw, _ = self.evidence("rsp", stack=True, stack_register=4)
        result = self.publish(archive, manifest, "refused-rsp", blocks=True)
        self.assertEqual(result["package_evidence"]["entries"], [])
        self.assertEqual(result["blocking_findings"], raw["blocking_findings"])

    def test_file_and_virtual_aliases_refuse_receipt_construction(self):
        overlaps = {
            "virtual": (0x540, ADDRESS + LITERAL, 16, 16),
            "partial-file": (LITERAL + 2, ADDRESS + 0x600, 4, 4),
            "zero-fill": (0x580, ADDRESS + LITERAL + 2, 0, 16),
            "inverted-size": (0x540, ADDRESS + 0x600, 16, 8),
        }
        for name, mapping in overlaps.items():
            with self.subTest(mapping=name):
                archive, manifest, raw, _ = self.evidence(name, extra_mapping=mapping)
                refused = self.publish(archive, manifest, f"refused-{name}", blocks=True)
                self.assertEqual(refused["package_evidence"]["entries"], [])
                self.assertEqual(refused["blocking_findings"], raw["blocking_findings"])

    def test_independent_credential_finding_survives_cold_and_warm_receipts(self):
        archive, manifest, raw, reviewed = self.evidence("sibling", extra=packages.PUBLIC_SOURCE)
        cache = self.root / "go-view-cache.json"
        options = ["--detector-cache-context", "c" * 64, "--detector-cache-output", str(cache)]
        cold = self.publish(archive, manifest, "cold-go", blocks=True, options=options)
        warm = self.publish(archive, manifest, "warm-go", blocks=True,
                            options=[*options, "--detector-cache-input", str(cache)])
        self.assertEqual(warm["detector_cache"]["hits"], 1)
        self.assertEqual(cold["package_evidence"], warm["package_evidence"])
        self.assertEqual(cold["blocking_findings"], [item for item in raw["blocking_findings"]
                                                    if item["finding_id"] != reviewed])
