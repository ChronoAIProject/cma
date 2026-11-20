"""Recognize the exact ensurepip certificate bundle without authorizing its disposition."""

from .trust_store import certificate_inventory

COMPONENT = "runtime-omarchy"
WHEEL = "usr/lib/python3.14/ensurepip/_bundled/pip-26.2.1-py3-none-any.whl"
MEMBER = "pip/_vendor/certifi/cacert.pem"
PATH = WHEEL + "!" + MEMBER
COUNT = 118


def inspect_bundle(physical, inspect_der):
    return certificate_inventory(physical, inspect_der, COUNT, comments=True)
