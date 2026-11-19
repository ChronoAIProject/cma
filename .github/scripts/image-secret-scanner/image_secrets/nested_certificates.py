"""Recognize the exact ensurepip certificate bundle without authorizing its disposition."""

from .common import digest_bytes
from .trust_store import certificate_digest

COMPONENT = "runtime-omarchy"
WHEEL = "usr/lib/python3.14/ensurepip/_bundled/pip-26.2.1-py3-none-any.whl"
MEMBER = "pip/_vendor/certifi/cacert.pem"
PATH = WHEEL + "!" + MEMBER
COUNT = 118


def inspect_bundle(physical, inspect_der):
    hashes = []

    def observe(der):
        inspect_der(der)
        hashes.append(digest_bytes(der))

    member_digest = certificate_digest(physical, observe, pem_bundle_limit=COUNT)
    if member_digest and len(set(hashes)) == COUNT:
        return {"member_sha256": member_digest, "der_sha256s": sorted(hashes)}
    return None
