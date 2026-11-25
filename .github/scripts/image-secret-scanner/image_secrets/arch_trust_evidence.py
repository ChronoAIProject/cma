"""Validate the authenticated inputs for the Arch trust-store decisions."""

from . import arch_trust
from .gate import valid_digest
from .package_evidence import LAYER_ID, NESTED_LOCATOR, fields, matches, normalized, ordinal, require


def verify_gnupg(row, packages):
    native = row["native"]
    fields(native, {"package_sha256", "member", "mtree_sha256"})
    digest = native["package_sha256"]
    require(valid_digest(digest) and digest in packages and valid_digest(native["mtree_sha256"]))
    package = packages[digest]
    require(package["name"] == "gnupg" and package["authority_type"] == "arch-signature"
            and package["architecture"] == "x86_64")
    member = native["member"]
    fields(member, {"locator", "path", "type", "sha256"})
    require(matches(NESTED_LOCATOR, member["locator"]) and member["type"] == "regular"
            and member["sha256"] == row["file_sha256"] and isinstance(member["path"], list)
            and len(member["path"]) == member["locator"].count("/") + 1)
    require([normalized(part) for part in member["path"]][-1] == arch_trust.GNUPG)


def load(raw, packages):
    keys = {"base", "files", "alias"}
    require(isinstance(raw, dict) and raw.keys() in (keys, keys | {"repack"}))
    base = raw["base"]
    fields(base, {"index_digest", "platform_manifest_digest", "layer_diff_id"})
    require(all(matches(LAYER_ID, digest) for digest in base.values()))
    require(isinstance(raw["files"], list) and 1 <= len(raw["files"]) <= len(arch_trust.FILES))
    files = {}
    entries = set()
    for record in raw["files"]:
        fields(record, {"path", "entry", "finding_id", "file_sha256", "der_sha256s", "proof_sha256", "native"})
        path, entry = record["path"], record["entry"]
        require(isinstance(path, str) and path in arch_trust.FILES and path not in files
                and ordinal(entry) and entry not in entries
                and all(valid_digest(record[key]) for key in ("finding_id", "file_sha256", "proof_sha256")))
        hashes = record["der_sha256s"]
        require(isinstance(hashes, list) and len(hashes) == arch_trust.FILES[path]
                and all(valid_digest(value) for value in hashes) and hashes == sorted(set(hashes)))
        if path == arch_trust.GNUPG:
            verify_gnupg(record, packages)
        else:
            require(record["native"] is None)
        files[path] = record
        entries.add(entry)
    alias = raw["alias"]
    if alias is not None:
        fields(alias, {"path", "entry", "finding_id", "link_target", "proof_sha256"})
        require(alias["path"] == arch_trust.ALIAS and alias["link_target"] == arch_trust.LINK
                and arch_trust.TLS in files and ordinal(alias["entry"]) and alias["entry"] not in entries
                and all(valid_digest(alias[key]) for key in ("finding_id", "proof_sha256")))
    packet = {"base": base, "files": files, "alias": alias}
    if "repack" in raw:
        repack = raw["repack"]
        fields(repack, {"archive_sha256", "image_config", "image_manifest", "layer_diff_id",
                        "producer_sha256", "recipe_sha256", "attestation_sha256"})
        require(all(valid_digest(repack[key]) for key in ("archive_sha256", "producer_sha256",
                                                          "recipe_sha256", "attestation_sha256"))
                and all(matches(LAYER_ID, repack[key]) for key in ("image_config", "layer_diff_id"))
                and (repack["image_manifest"] is None or matches(LAYER_ID, repack["image_manifest"]))
                and repack["layer_diff_id"] != base["layer_diff_id"])
        packet["repack"] = repack
    return packet


def load_portable(raw, packages):
    """Rebuild-stable form: exact paths, bytes, DER inventories and the link; no image or finding identity."""
    fields(raw, {"files", "alias"})
    require(isinstance(raw["files"], list) and 1 <= len(raw["files"]) <= len(arch_trust.FILES))
    files = {}
    for record in raw["files"]:
        fields(record, {"path", "file_sha256", "certificates", "der_sha256s", "proof_sha256", "native"})
        path, count, hashes = record["path"], record["certificates"], record["der_sha256s"]
        require(isinstance(path, str) and path in arch_trust.FILES and path not in files
                and type(count) is int and count == arch_trust.FILES[path]
                and valid_digest(record["file_sha256"]) and valid_digest(record["proof_sha256"]))
        require(isinstance(hashes, list) and len(hashes) == count
                and all(valid_digest(value) for value in hashes) and hashes == sorted(set(hashes)))
        if path == arch_trust.GNUPG:
            verify_gnupg(record, packages)
        else:
            require(record["native"] is None)
        files[path] = record
    alias = raw["alias"]
    if alias is not None:
        fields(alias, {"path", "link_target", "proof_sha256"})
        require(alias["path"] == arch_trust.ALIAS and alias["link_target"] == arch_trust.LINK
                and arch_trust.TLS in files and valid_digest(alias["proof_sha256"]))
    return {"files": files, "alias": alias}


def load_catalog(raw, packages):
    packets = raw if isinstance(raw, list) else [raw]
    require(0 < len(packets) <= 16)
    indexed = {}
    for item in packets:
        packet = load(item, packages)
        layer = arch_trust.physical_layer(packet)
        require(layer not in indexed)
        indexed[layer] = packet
    return indexed
