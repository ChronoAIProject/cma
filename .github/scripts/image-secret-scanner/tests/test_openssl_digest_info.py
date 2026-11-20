"""Keep OpenSSL's public DigestInfo table distinct from binary private keys."""

import subprocess
import unittest

from image_secrets.key_material import binary_key_spans, has_binary_key
from tests import test_trust_store as trust_tests
from tests.test_library_vectors import ELF

# Bytes compiled from crypto/rsa/rsa_sign.c into Alpine libcrypto3 3.5.6-r0 and
# 3.5.9-r0: the SHA-256, SHA-1, SM3, RIPEMD-160, MD5 and MD4 DigestInfo prefixes
# with their zero padding, followed by two bytes of the next string.
TABLE = bytes.fromhex(
    "3031300d06096086480165030402010500042000000000003021300906052b0e"
    "03021a050004140000000000000000003030300c06082a811ccf550183780500"
    "04200000000000003021300906052b2403020105000414000000000000000000"
    "3020300c06082a864886f70d0205050004100000000000000000000000000000"
    "3020300c06082a864886f70d0203050004100000000000000000000000000000"
    "6f73"
)
SM3 = TABLE[0x30:0x62]
MD4 = TABLE[0x80:0xa2]


def frame(oid, digest_size, parameters=b"\x05\x00"):
    algorithm = b"\x06" + bytes([len(oid)]) + oid + parameters
    child = b"\x30" + bytes([len(algorithm)]) + algorithm
    body = child + b"\x04" + bytes([digest_size]) + bytes(digest_size)
    return b"\x30" + bytes([len(body)]) + body


class OpenSSLDigestInfoTests(unittest.TestCase):
    setUp = trust_tests.TrustStoreTests.setUp
    tearDown = trust_tests.TrustStoreTests.tearDown
    scan = trust_tests.TrustStoreTests.scan
    material = trust_tests.TrustStoreTests.material
    image_scan = trust_tests.TrustStoreTests.image_scan

    def test_compiled_openssl_table_has_no_binary_key(self):
        self.assertEqual((len(SM3), len(MD4)), (50, 34))
        self.assertEqual(list(binary_key_spans(TABLE)), [])
        for exact in (SM3, MD4):
            self.assertFalse(has_binary_key(exact))
        sm3_oid, md4_oid = SM3[6:14], MD4[6:14]
        self.assertEqual(frame(sm3_oid, 32)[:18], SM3[:18])
        self.assertEqual(frame(md4_oid, 16)[:18], MD4[:18])
        # Absent parameters remain the other standard DigestInfo encoding.
        self.assertFalse(has_binary_key(frame(sm3_oid, 32, b"")))
        self.assertFalse(has_binary_key(frame(md4_oid, 16, b"")))

    def test_other_digest_sizes_parameters_or_algorithms_still_block(self):
        sm3_oid, md4_oid = SM3[6:14], MD4[6:14]
        unlisted = bytes.fromhex("2a864886f70d0206")
        for blocked in (frame(sm3_oid, 33), frame(sm3_oid, 16), frame(md4_oid, 32), frame(md4_oid, 20),
                        frame(sm3_oid, 32, b"\x01\x01\x00"), frame(md4_oid, 16, b"\x04\x00"),
                        frame(unlisted, 16), frame(unlisted, 32)):
            with self.subTest(blocked=blocked.hex()):
                self.assertTrue(has_binary_key(blocked))

    def test_generated_private_keys_beside_the_table_still_block(self):
        _, _, _, pkcs8, encrypted = self.material()
        ec = subprocess.run(["openssl", "genpkey", "-algorithm", "EC", "-pkeyopt", "ec_paramgen_curve:P-256",
                             "-outform", "DER"], capture_output=True, check=True, timeout=15).stdout
        for key in (pkcs8, encrypted, ec):
            with self.subTest(size=len(key)):
                spans = list(binary_key_spans(TABLE + key))
                self.assertEqual(spans[:1], [(len(TABLE), len(key))])
                self.assertTrue(has_binary_key(key + TABLE))

    def test_image_scan_clears_the_table_but_blocks_a_key(self):
        library = ELF + TABLE
        clean = self.image_scan([[("usr/lib/libcrypto.so.3", library)]], 0, component=None)
        self.assertNotIn("content.binary-private-key", {row["rule"] for row in clean["findings"]})
        _, _, _, pkcs8, _ = self.material()
        refused = self.image_scan([[("usr/lib/libcrypto.so.3", library + pkcs8)]], 1, component=None)
        self.assertEqual([row["rule"] for row in refused["blocking_findings"]], ["content.binary-private-key"])
