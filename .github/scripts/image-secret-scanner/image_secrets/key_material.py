"""Recognize binary private-key containers independently of image paths."""

import mmap
import re
import subprocess

from .common import ScanError

# DER frames identify candidates only. Unencrypted candidates must also be
# accepted by OpenSSL's private-key decoder. Its stdout/stderr never escape.
LENGTH = rb"(?:[\x00-\x7f]|\x81.|\x82..|\x83...|\x84....)"
START = re.compile(rb"\x30" + LENGTH + rb"(?:\x02\x01[\x00\x01][\x02\x04\x30]|\x30" +
                   LENGTH + rb"\x06)", re.DOTALL)
OPENSSH = b"openssh-key-v1\x00"
LIMIT = 1024 * 1024
# RSA DigestInfo uses the same two outer ASN.1 types as EncryptedPrivateKeyInfo.
# Recognize only complete standard fixed-length hashes with absent/NULL parameters.
HASH_SIZES = {bytes.fromhex(oid): size for oid, size in {
    "2a864886f70d0202": 16, "2a864886f70d0204": 16, "2a864886f70d0205": 16,  # MD2/4/5
    "55080365": 16, "2b24030201": 20, "2b0e03021a": 20,  # MDC2, RIPEMD-160, SHA-1
}.items()}
NIST_HASH = bytes.fromhex("6086480165030402")
HASH_SIZES.update({NIST_HASH + bytes([i]): size for i, size in
                   enumerate((32, 48, 64, 28, 28, 32, 28, 32, 48, 64), start=1)})



def frame(data, at, end):
    """Return type, content start and exclusive end of a definite DER value."""
    if at + 2 > end:
        return None
    tag, size = data[at], data[at + 1]
    start = at + 2
    if size >= 128:
        count = size - 128
        if not 1 <= count <= 4 or start + count > end or data[start] == 0:
            return None
        size = int.from_bytes(data[start:start + count], "big")
        if size < 128:
            return None
        start += count
    return (tag, start, start + size) if start + size <= end else None


def decode(args, data):
    try:
        return subprocess.run(["openssl", *args], input=data, stdout=subprocess.PIPE,
                              stderr=subprocess.DEVNULL, timeout=5, check=False)
    except (OSError, subprocess.TimeoutExpired) as exc:
        raise ScanError("key-material-parser-unavailable") from exc


def private_der(data, at):
    outer = frame(data, at, len(data))
    if not outer or outer[0] != 48:
        return False
    end = outer[2]
    child = frame(data, outer[1], end)
    if not child:
        return False
    if child[0] == 48:
        # EncryptedPrivateKeyInfo is an AlgorithmIdentifier and ciphertext.
        # Identifying this envelope must never require a decryption password.
        oid = frame(data, child[1], child[2])
        cipher = frame(data, child[2], end)
        if not (oid and oid[0] == 6 and oid[2] > oid[1] and cipher and cipher[0] == 4
                and cipher[1] < cipher[2] == end):
            return False
        # AlgorithmIdentifier permits at most one parameter, and the OID must be
        # a sequence of complete, minimally encoded base-128 subidentifiers.
        if oid[2] - oid[1] > 128:
            return True  # Unmodeled algorithm, retain the blocker without an unbounded allocation.
        ident = data[oid[1]:oid[2]]
        if ident[-1] & 0x80 or ident[0] == 0x80 or any(
                current == 0x80 and not previous & 0x80 for previous, current in zip(ident, ident[1:])):
            return False
        params = frame(data, oid[2], child[2]) if oid[2] < child[2] else None
        if oid[2] != child[2] and (not params or params[2] != child[2]):
            return False
        no_parameters = params is None or (params[0] == 5 and params[1] == params[2])
        return not (no_parameters and HASH_SIZES.get(ident) == cipher[2] - cipher[1])
    fields = []
    pos = child[2]
    while pos < end and len(fields) < 12:
        field = frame(data, pos, end)
        if not field:
            return False
        fields.append(field[0])
        pos = field[2]
    if pos != end or not (fields[:2] == [48, 4] or fields[:1] == [4]
                          or (len(fields) >= 5 and fields[:5] == [2] * 5)):
        return False
    if end - at > LIMIT:
        # A plausibly framed oversized key cannot be qualified as public.
        return True
    return decode(["pkey", "-inform", "DER", "-noout", "-passin", "pass:"], data[at:end]).returncode == 0


def openssh_key(data, at):
    pos = at + len(OPENSSH)

    def string(start):
        if start + 4 > len(data):
            return None
        end = start + 4 + int.from_bytes(data[start:start + 4], "big")
        return end if end <= len(data) else None

    for _ in range(3):
        pos = string(pos)
        if pos is None:
            return False
    if pos + 4 > len(data):
        return False
    count = int.from_bytes(data[pos:pos + 4], "big")
    pos += 4
    if not 0 < count <= 1024:
        return False
    for _ in range(count + 1):
        end = string(pos)
        if end is None or end <= pos + 4:
            return False
        pos = end
    return True


def has_binary_key(data):
    start = data.find(OPENSSH)
    while start >= 0:
        if openssh_key(data, start):
            return True
        start = data.find(OPENSSH, start + len(OPENSSH))
    return any(private_der(data, match.start()) for match in START.finditer(data))


def binary_key_in_file(path):
    # Inspect the whole candidate after a streaming signature hit so DER length
    # and key bytes can cross read boundaries, without loading image binaries.
    with path.open("rb") as source:
        with mmap.mmap(source.fileno(), 0, access=mmap.ACCESS_READ) as data:
            return has_binary_key(data)
