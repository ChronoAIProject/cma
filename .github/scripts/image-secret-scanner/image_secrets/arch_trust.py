"""Exact Omarchy certificates and the generated Arch trust-store alias."""

from .trust_store import certificate_inventory

COMPONENT = "runtime-omarchy"
PLATFORM = "linux/amd64"
LAYER = 0
EMAIL = "etc/ca-certificates/extracted/email-ca-bundle.pem"
TLS = "etc/ca-certificates/extracted/tls-ca-bundle.pem"
GNUPG = "usr/share/gnupg/sks-keyservers.netCA.pem"
ALIAS = "etc/ssl/cert.pem"
LINK = "../ca-certificates/extracted/tls-ca-bundle.pem"
FILES = {EMAIL: 88, TLS: 118, GNUPG: 1}


def inspect_file(physical, path, inspect_der):
    return certificate_inventory(physical, inspect_der, FILES[path], comments=path != GNUPG)


def observe_file(physical, location, stage, observations):
    path = location["path"]
    checked = inspect_file(physical, path, lambda der: stage.certificate(der, location))
    if checked:
        coordinate = {key: location[key] for key in ("image", "layer", "entry")}
        observations.append(dict(coordinate, **checked))
