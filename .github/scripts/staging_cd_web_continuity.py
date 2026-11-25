"""Validate the Web asset continuity proof carried by an immutable release."""

from staging_cd_common import matches, require
from staging_cd_receipts import sha256, validate_receipt
from staging_cd_versions import version_tuple


SCHEMA = "cma-web-asset-continuity-v1"
PROOF_SCHEMA = "cma-web-asset-publication-proof-v1"
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


def require_images(value, proof, registry):
    incumbent = value["incumbent"]
    images = [incumbent["image"], value["prepare_image"], *(item["image"] for item in proof["inputs"])]
    for image in dict.fromkeys(images):
        require(registry.exists(REPOSITORY, image.split("@")[1]) is True,
                "Web continuity image is not publicly available")


def binding(value):
    return sha256({key: field for key, field in value.items() if key != "proof_sha256"})


def validate_proof(proof, continuity, identity, promote_image, promote_archive):
    """Bind the exact detached evidence from the immutable Web release annotation."""
    require(isinstance(proof, dict) and set(proof) == {
        "schema", "continuity_sha256", "retention_preimage_sha256", "asset_count", "asset_bytes",
        "inputs", "overlays",
    } and proof["schema"] == PROOF_SCHEMA and sha256(proof) == continuity["proof_sha256"],
            "Web continuity publication proof hash differs")
    require(proof["continuity_sha256"] == binding(continuity)
            and matches(HASH, proof["retention_preimage_sha256"])
            and all(type(proof[key]) is int and proof[key] > 0 for key in ("asset_count", "asset_bytes")),
            "Web continuity publication proof scope differs")
    inputs = proof["inputs"]
    require(isinstance(inputs, list) and len(inputs) >= 2, "Web continuity publication inputs are incomplete")
    images = {}
    for row in inputs:
        require(isinstance(row, dict) and set(row) == {"image", "source_sha", "version", "catalog_sha256"}
                and matches(IMAGE, row["image"]) and not row["image"].endswith("0" * 64)
                and matches(r"[0-9a-f]{40}", row["source_sha"])
                and matches(HASH, row["catalog_sha256"]) and row["image"] not in images,
                "Web continuity publication input is malformed")
        version_tuple(row["version"])
        images[row["image"]] = (row["source_sha"], row["version"])
    require(list(images) == sorted(images) and len(set(images.values())) == len(images),
            "Web continuity publication inputs are ambiguous")
    incumbent = continuity["incumbent"]
    candidate = continuity["candidate_base_image"]
    require(images.get(incumbent["base_image"]) == (incumbent["source_sha"], incumbent["version"])
            and images.get(candidate) == (identity["source"], identity["version"]),
            "Web continuity publication bases differ")
    overlays = proof["overlays"]
    require(isinstance(overlays, dict) and set(overlays) == {"prepare", "promote"},
            "Web continuity publication overlay proofs are incomplete")
    expected = {"prepare": (continuity["prepare_image"], incumbent["base_image"]), "promote": (promote_image, candidate)}
    for role, (image, base) in expected.items():
        row = overlays[role]
        require(isinstance(row, dict) and set(row) == {
            "image", "base_image", "layer_digest", "archive_sha256", "credential_report_sha256",
        } and row["image"] == image and row["base_image"] == base
            and matches("sha256:" + HASH, row["layer_digest"])
            and all(matches(HASH, row[key]) for key in ("archive_sha256", "credential_report_sha256")),
                "Web continuity publication overlay proof differs")
    require(overlays["prepare"]["layer_digest"] == overlays["promote"]["layer_digest"]
            and overlays["promote"]["archive_sha256"] == promote_archive,
            "Web continuity publication archive or shared layer differs")


def retained_inputs(incumbent, finalized, *, allow_bootstrap=False):
    """Resolve raw lineage for a settled serving image against verified final Web tags."""
    matches_serving = []
    for receipt in finalized:
        validate_receipt(receipt)
        require(receipt["identity"]["component"] == "web", "Web continuity lineage includes a different component")
        promote = receipt["repository"] + "@" + receipt["digest"]
        continuity = receipt.get("asset_continuity")
        if promote == incumbent["image"]:
            base = continuity["candidate_base_image"] if continuity else promote
            expected = {"image": promote, "base_image": base, "source_sha": receipt["source"],
                        "version": receipt["identity"]["version"]}
        elif continuity and continuity["prepare_image"] == incumbent["image"]:
            expected = dict(continuity["incumbent"], image=continuity["prepare_image"])
        else:
            continue
        require(expected == incumbent, "Serving Web identity differs from its finalized lineage")
        matches_serving.append(receipt.get("asset_proof"))
    require(len(matches_serving) == 1, "Serving Web image has no unique finalized lineage")
    [proof] = matches_serving
    if proof is None:
        require(allow_bootstrap is True, "An unprotected Web image cannot supply retained lineage")
        return []
    return [dict(row) for row in proof["inputs"] if row["image"] != incumbent["base_image"]]
