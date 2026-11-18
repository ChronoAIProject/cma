"""Strict public release receipts bind recovery to the exact published image."""
import hashlib
import json

from staging_cd_common import matches, require
from staging_cd_versions import DEPLOYABLES, LEVELS, bump, version_tuple

IDENTITY_LABEL = "dev.chrono.cma.release"
SCANNER_BUNDLE = "c84f531cc76a09aca96ba5fa153d31a640e119fb27b9c0c73b3a9d538c94f9df"
IDENTITY_FIELDS = {"schema", "component", "version", "previous", "source", "tree", "pr", "bump",
                   "run_id", "run_attempt", "build"}
RECEIPT_FIELDS = {"schema", "identity", "tag", "repository", "source", "digest", "archive", "scan"}


def canonical(value):
    return json.dumps(value, sort_keys=True, separators=(",", ":"))


def sha256(value):
    return hashlib.sha256(canonical(value).encode()).hexdigest()


def validate_identity(identity):
    require(isinstance(identity, dict) and set(identity) == IDENTITY_FIELDS
            and identity["schema"] == "cma-image-release-v1", "Image release identity is malformed")
    require(identity["component"] in DEPLOYABLES and identity["bump"] in LEVELS.values(),
            "Image release component or bump is invalid")
    version_tuple(identity["previous"])
    require(bump(identity["previous"], identity["bump"]) == identity["version"],
            "Image release version does not match its frozen bump")
    require(all(matches(r"[0-9a-f]{40}", identity[key]) for key in ("source", "tree"))
            and matches(r"[0-9a-f]{64}", identity["build"]), "Image release hashes are invalid")
    require(all(type(identity[key]) is int and identity[key] > 0 for key in ("pr", "run_id", "run_attempt")),
            "Image release provenance is invalid")
    return identity


def validate_receipt(receipt):
    require(isinstance(receipt, dict) and set(receipt) == RECEIPT_FIELDS
            and receipt["schema"] == "cma-published-image-v1", "Published receipt is malformed")
    identity = validate_identity(receipt["identity"])
    name = DEPLOYABLES[identity["component"]]
    require(receipt["tag"] == name + "/v" + identity["version"]
            and receipt["repository"] == "ghcr.io/chronoaiproject/" + name
            and receipt["source"] == identity["source"], "Published receipt identity differs")
    require(matches(r"sha256:[0-9a-f]{64}", receipt["digest"])
            and matches(r"[0-9a-f]{64}", receipt["archive"]), "Published image digest is malformed")
    scan = receipt["scan"]
    require(isinstance(scan, dict) and set(scan) == {"version", "bundle", "mode", "complete", "counts"}
            and scan["version"] == "1.0.3" and scan["bundle"] == SCANNER_BUNDLE
            and scan["mode"] == "credential" and scan["complete"] is True,
            "Published receipt does not prove the reviewed Policy A scan")
    counts = scan["counts"]
    require(isinstance(counts, dict) and set(counts) == {"detected", "blocking", "advisory", "reviewed", "allowlisted"}
            and all(type(n) is int and n >= 0 for n in counts.values())
            and counts["blocking"] == counts["allowlisted"] == 0
            and counts["detected"] == counts["advisory"] + counts["reviewed"],
            "Published receipt scan counts are invalid")
    return receipt


def published_receipt(identity, digest, archive, counts):
    validate_identity(identity)
    name = DEPLOYABLES[identity["component"]]
    return validate_receipt({"schema": "cma-published-image-v1", "identity": identity,
                             "tag": name + "/v" + identity["version"],
                             "repository": "ghcr.io/chronoaiproject/" + name,
                             "source": identity["source"], "digest": digest, "archive": archive,
                             "scan": {"version": "1.0.3", "bundle": SCANNER_BUNDLE,
                                      "mode": "credential", "complete": True, "counts": counts}})


def annotation(receipt):
    return canonical(validate_receipt(receipt)) + "\n"


def parse_annotation(message):
    require(isinstance(message, str) and len(message) <= 16384, "Version tag annotation is invalid")
    def unique(pairs):
        result = {}
        for key, value in pairs:
            require(key not in result, "Duplicate release receipt field")
            result[key] = value
        return result
    try:
        receipt = json.loads(message, object_pairs_hook=unique)
    except (ValueError, TypeError) as error:
        raise ValueError("Version tag does not contain a release receipt") from error
    validate_receipt(receipt)
    require(message.strip() == canonical(receipt), "Version tag annotation is not canonical")
    return receipt

