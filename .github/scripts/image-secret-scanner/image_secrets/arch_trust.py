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


def physical_layer(packet):
    repack = packet.get("repack")
    return packet["base"]["layer_diff_id"] if repack is None else repack["layer_diff_id"]


def matches_image(packet, archive, config, manifest):
    repack = packet.get("repack")
    return repack is None or (repack["archive_sha256"], repack["image_config"], repack["image_manifest"]) == (
        archive, config, manifest)


def portable_entries(packet):
    """Reviewed rebuild-stable paths that the receipt reopens in the base layer by name."""
    entries = dict(packet["files"])
    if packet["alias"] is not None:
        entries[ALIAS] = packet["alias"]
    return entries


def resolves(namespace, path):
    """Checked after every layer: the alias reaches the TLS bundle, and each file is never replaced."""
    if path == ALIAS:
        return namespace.direct_alias(ALIAS, TLS, LAYER)
    return namespace.final_entry(path, LAYER)


def inspect_file(physical, path, inspect_der):
    return certificate_inventory(physical, inspect_der, FILES[path], comments=path != GNUPG)


def observe_file(physical, location, stage, observations):
    path = location["path"]
    checked = inspect_file(physical, path, lambda der: stage.certificate(der, location))
    if checked:
        coordinate = {key: location[key] for key in ("image", "layer", "entry")}
        observations.append(dict(coordinate, **checked))
