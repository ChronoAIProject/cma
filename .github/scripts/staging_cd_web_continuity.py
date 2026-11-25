"""Validate the Web asset continuity proof carried by an immutable release."""

from staging_cd_common import matches, require
from staging_cd_versions import version_tuple


SCHEMA = "cma-web-asset-continuity-v1"
REPOSITORY = "ghcr.io/chronoaiproject/cma-web"
IMAGE = r"ghcr\.io/chronoaiproject/cma-web@sha256:[0-9a-f]{64}"
HASH = r"[0-9a-f]{64}"
ASSET = r"/assets/[A-Za-z0-9_-][A-Za-z0-9_.-]{0,239}"


def validate(value, identity, promote_image):
    require(isinstance(value, dict) and set(value) == {
        "schema", "incumbent", "candidate_base_image", "prepare_image", "asset_manifest_sha256",
        "proof_sha256", "sentinels",
    } and value["schema"] == SCHEMA, "Web continuity receipt is malformed")
    incumbent = value["incumbent"]
    require(isinstance(incumbent, dict) and set(incumbent) == {
        "image", "base_image", "source_sha", "version",
    } and matches(r"[0-9a-f]{40}", incumbent["source_sha"])
        and incumbent["source_sha"] != identity["source"], "Web continuity incumbent is malformed")
    version_tuple(incumbent["version"])
    require(incumbent["version"] != identity["version"], "Web continuity incumbent version differs")
    images = (incumbent["image"], incumbent["base_image"], value["candidate_base_image"],
              value["prepare_image"], promote_image)
    require(all(matches(IMAGE, image) and not image.endswith("0" * 64) for image in images)
            and len(set(images[2:])) == 3 and not set(images[:2]).intersection(images[2:]),
            "Web continuity image identities overlap or are malformed")
    require(all(matches(HASH, value[key]) for key in ("asset_manifest_sha256", "proof_sha256")),
            "Web continuity proof hash is malformed")
    sentinels = value["sentinels"]
    require(isinstance(sentinels, dict) and set(sentinels) == {"incumbent", "candidate"},
            "Web continuity sentinels are incomplete")
    for row in sentinels.values():
        require(isinstance(row, dict) and set(row) == {"path", "sha256", "html_sha256"}
                and matches(ASSET, row["path"]) and matches(HASH, row["sha256"])
                and matches(HASH, row["html_sha256"]), "Web continuity sentinel is malformed")
    old, candidate = sentinels["incumbent"], sentinels["candidate"]
    require(old["path"] != candidate["path"] or old["sha256"] == candidate["sha256"],
            "Web continuity sentinel path has conflicting bytes")
    return value


def require_images(value, registry):
    incumbent = value["incumbent"]
    for image in dict.fromkeys((incumbent["image"], incumbent["base_image"],
                                value["candidate_base_image"], value["prepare_image"])):
        require(registry.exists(REPOSITORY, image.split("@")[1]) is True,
                "Web continuity image is not publicly available")
