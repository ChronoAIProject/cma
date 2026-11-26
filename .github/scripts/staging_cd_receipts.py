"""Strict public release receipts bind recovery to the exact published image."""
import hashlib
import json

from deployables import known_components
from image_scanner_policy import MANIFEST_SHA256

from staging_cd_common import matches, require
from staging_cd_versions import DEPLOYABLES, LEVELS, bump, version_tuple

KNOWN_DEPLOYABLES = known_components()
IDENTITY_LABEL = "dev.chrono.cma.release"
# Exact reader pairs retain immutable historical tags across both staging tracks.
# New publications must also match the manifest verified by this checkout.
SUPPORTED_SCANS = frozenset({
    ("1.0.3", "c84f531cc76a09aca96ba5fa153d31a640e119fb27b9c0c73b3a9d538c94f9df"),
    ("1.0.3", "0d9e8ed1bd80b7e055107023fe73ce7c5cebb241c91667c07157d39c775b8aa3"),
    ("1.0.3", "1d7bc6478a8e4c8aa11c4e7293403ce5128efde58dec3206cf2d219e896a9dce"),
    ("1.0.4", "06e96df755ff3d8a60894bac93cfc9f80e713b44024b914b963f60ba00d7d2aa"),
    ("1.0.4", "7f21900612a77b2238201ee4e284b2c3567e7d86708da75d78cb5b9e56f2f40d"),
    ("1.0.4", "9681fc9bd65741c184fce94825a58083ef2acdf66e3b2f14216715a29a681660"),
    ("1.0.4", "312a00d5d22b8d04faf7483724891fc449efadc4abec152a0c9c3dcb2a63464d"),
    ("1.0.4", "eabea91e1c449292aecf68f8a33e343032d9758e36e89e2275fe81b09b8a2bca"),
    ("1.0.4", "61861609136340969ec648836cb35cb5edd1988addf8e94d6725f262841d5258"),
    ("1.0.4", "af54d675d4a03726bf91af02a3a44fa06953f0abc325a29771949efc8b95343e"),
    ("1.0.4", "b663fdcbe2e0b606f1c176bf1b79d215d444a621039b1db907ef0c3767e6b5b1"),
    ("1.0.4", "3d562617c65f85ae9fd958adfbf76e1e38e18a135f95ad2209348be42fa3a810"),
    ("1.0.4", "791cef403329aa642586b3d3d937109524b98f87a3b09064c303c4aadbcc0051"),
    ("1.0.4", "a3eb9041f1b920bda1f8b2bd63241ea5ed0c890f60841a7303670b35dbb384c3"),
    ("1.0.4", "4ad252394ac8d144da7b316f138c71f90ebf9578bce9f3d118b77bee7b8da11c"),
})
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
    require(identity["component"] in KNOWN_DEPLOYABLES and identity["bump"] in LEVELS.values(),
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
    require(isinstance(receipt, dict), "Published receipt is malformed")
    fields = RECEIPT_FIELDS | ({"compatibility"} if receipt.get("schema") == "cma-published-image-v2" else set())
    require(set(receipt) in (fields, fields | {"runtime_smoke"})
            and receipt["schema"] in ("cma-published-image-v1", "cma-published-image-v2"),
            "Published receipt is malformed")
    identity = validate_identity(receipt["identity"])
    if "compatibility" in receipt:
        from staging_cd_contract import GROUP, validate
        require(identity["component"] in GROUP, "Unexpected compatibility receipt")
        validate(receipt["compatibility"], identity["source"])
    name = KNOWN_DEPLOYABLES[identity["component"]]
    require(receipt["tag"] == name + "/v" + identity["version"]
            and receipt["repository"] == "ghcr.io/chronoaiproject/" + name
            and receipt["source"] == identity["source"], "Published receipt identity differs")
    require(matches(r"sha256:[0-9a-f]{64}", receipt["digest"])
            and matches(r"[0-9a-f]{64}", receipt["archive"]), "Published image digest is malformed")
    scan = receipt["scan"]
    require(isinstance(scan, dict) and set(scan) == {"version", "bundle", "mode", "complete", "counts"}
            and isinstance(scan["version"], str) and isinstance(scan["bundle"], str)
            and (scan["version"], scan["bundle"]) in SUPPORTED_SCANS
            and scan["mode"] == "credential" and scan["complete"] is True,
            "Published receipt does not prove the reviewed Policy A scan")
    counts = scan["counts"]
    require(isinstance(counts, dict) and set(counts) == {"detected", "blocking", "advisory", "reviewed", "allowlisted"}
            and all(type(n) is int and n >= 0 for n in counts.values())
            and counts["blocking"] == counts["allowlisted"] == 0
            and counts["detected"] == counts["advisory"] + counts["reviewed"],
            "Published receipt scan counts are invalid")
    if "runtime_smoke" in receipt:
        from staging_cd_runtime_smoke import validate_smoke
        from staging_cd_targets import RUNTIME_TARGETS
        require(identity["component"] in RUNTIME_TARGETS, "Unexpected runtime smoke receipt")
        validate_smoke(receipt["runtime_smoke"], source=receipt["source"],
                       digest=receipt["digest"], archive=receipt["archive"])
    return receipt


def published_receipt(identity, digest, archive, counts, *, scanner_identity, compatibility=None):
    require(isinstance(scanner_identity, tuple) and len(scanner_identity) == 2
            and all(isinstance(value, str) for value in scanner_identity)
            and scanner_identity in SUPPORTED_SCANS and scanner_identity[1] == MANIFEST_SHA256,
            "Fresh publication requires the current verified scanner identity")
    version, bundle = scanner_identity
    validate_identity(identity)
    require(identity["component"] in DEPLOYABLES, "Retired images cannot be published")
    name = KNOWN_DEPLOYABLES[identity["component"]]
    return validate_receipt({"schema": "cma-published-image-v2" if compatibility else "cma-published-image-v1", "identity": identity,
                             **({"compatibility": compatibility} if compatibility is not None else {}),
                             "tag": name + "/v" + identity["version"],
                             "repository": "ghcr.io/chronoaiproject/" + name,
                             "source": identity["source"], "digest": digest, "archive": archive,
                             "scan": {"version": version, "bundle": bundle,
                                      "mode": "credential", "complete": True, "counts": counts}})


def annotation(receipt):
    return canonical(validate_receipt(receipt)) + "\n"


def final_tag_text(receipt):
    # Stable tagger identity and timestamp allow a later attempt to recover
    # the writer's exact object ID from the original publication receipt.
    return (f"object {receipt['source']}\ntype commit\ntag {receipt['tag']}\n"
            "tagger github-actions[bot] <41898282+github-actions[bot]@users.noreply.github.com> 0 +0000\n\n"
            + annotation(receipt))


def git_tag_oid(raw):
    return hashlib.sha1(b"tag " + str(len(raw)).encode() + b"\0" + raw).hexdigest()


def final_tag_oid(receipt):
    return git_tag_oid(final_tag_text(receipt).encode())


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

