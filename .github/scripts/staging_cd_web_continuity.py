"""Validate the Web asset continuity proof carried by an immutable release."""

from staging_cd_clock import elapsed
from staging_cd_common import matches, require
from staging_cd_receipts import sha256, validate_receipt
from staging_cd_versions import version_tuple


SCHEMA = "cma-web-asset-continuity-v1"
PROOF_SCHEMA = "cma-web-asset-publication-proof-v1"
PREIMAGE_SCHEMA = "cma-web-continuity-preimage-v1"
BOOTSTRAP_SCHEMA = "cma-web-continuity-bootstrap-v1"
BOOTSTRAP_EVIDENCE_SCHEMA = "cma-web-continuity-bootstrap-evidence-v1"
REPOSITORY = "ghcr.io/chronoaiproject/cma-web"
IMAGE = r"ghcr\.io/chronoaiproject/cma-web@sha256:[0-9a-f]{64}"
HASH = r"[0-9a-f]{64}"
ASSET = r"/assets/[A-Za-z0-9_-][A-Za-z0-9_.-]{0,239}"
INPUT_FIELDS = {"image", "source_sha", "version", "completed_rollouts"}
DEFAULT_RETENTION_ROLLOUTS = 6


def immutable_image(image):
    return matches(IMAGE, image) and not image.endswith("0" * 64)


def validate_incumbent(value):
    require(isinstance(value, dict) and set(value) == {"image", "base_image", "source_sha", "version"}
            and immutable_image(value["image"]) and immutable_image(value["base_image"])
            and matches(r"[0-9a-f]{40}", value["source_sha"]),
            "Web continuity incumbent is malformed")
    version_tuple(value["version"])
    return value


def validate_input(row, *, catalog_required):
    fields = set(row) if isinstance(row, dict) else set()
    require((fields == INPUT_FIELDS | {"catalog_sha256"} or not catalog_required and fields == INPUT_FIELDS)
            and immutable_image(row["image"]) and matches(r"[0-9a-f]{40}", row["source_sha"])
            and type(row["completed_rollouts"]) is int and row["completed_rollouts"] >= 0
            and ("catalog_sha256" not in row or matches(HASH, row["catalog_sha256"])),
            "Web publication scope input is malformed")
    version_tuple(row["version"])


def validate_scope(scope):
    require(isinstance(scope, dict) and set(scope) == {"incumbent", "inputs", "preimage_sha256"}
            and matches(HASH, scope["preimage_sha256"]) and isinstance(scope["inputs"], list),
            "Web publication scope is malformed")
    validate_incumbent(scope["incumbent"])
    for row in scope["inputs"]:
        validate_input(row, catalog_required=False)
    images = [row["image"] for row in scope["inputs"]]
    require(images == sorted(set(images)), "Web publication scope inputs are ambiguous")
    return scope


def validate(value, identity, promote_image):
    require(isinstance(value, dict) and set(value) == {
        "schema", "incumbent", "candidate_base_image", "prepare_image", "asset_manifest_sha256",
        "proof_sha256", "sentinels",
    } and value["schema"] == SCHEMA, "Web continuity receipt is malformed")
    incumbent = validate_incumbent(value["incumbent"])
    require(incumbent["source_sha"] != identity["source"] and incumbent["version"] != identity["version"],
            "Web continuity incumbent and candidate identities overlap")
    images = (incumbent["image"], incumbent["base_image"], value["candidate_base_image"],
              value["prepare_image"], promote_image)
    require(all(immutable_image(image) for image in images[2:])
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
        validate_input(row, catalog_required=True)
        require(row["image"] not in images, "Web continuity publication input is duplicated")
        images[row["image"]] = (row["source_sha"], row["version"], row["completed_rollouts"])
    require(list(images) == sorted(images) and len({value[:2] for value in images.values()}) == len(images),
            "Web continuity publication inputs are ambiguous")
    incumbent = continuity["incumbent"]
    candidate = continuity["candidate_base_image"]
    require(images.get(incumbent["base_image"]) == (incumbent["source_sha"], incumbent["version"], 0)
            and images.get(candidate) == (identity["source"], identity["version"], 0),
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


def bootstrap_inputs(bootstrap, evidence, incumbent, finalized):
    require(isinstance(bootstrap, dict)
            and set(bootstrap) == {"schema", "incumbent", "retained", "approved_deployments_sha256"}
            and bootstrap["schema"] == BOOTSTRAP_SCHEMA and bootstrap["incumbent"] == incumbent
            and isinstance(bootstrap["retained"], list) and matches(HASH, bootstrap["approved_deployments_sha256"]),
            "Unprotected Web image requires an approved bootstrap lineage")
    recognized = {receipt["repository"] + "@" + receipt["digest"]: {
        "image": receipt["repository"] + "@" + receipt["digest"], "source_sha": receipt["source"],
        "version": receipt["identity"]["version"],
    } for receipt in finalized if "asset_continuity" not in receipt}
    for row in bootstrap["retained"]:
        require(isinstance(row, dict) and immutable_image(row.get("image"))
                and row["image"] != incumbent["base_image"] and recognized.get(row["image"]) == row,
                "Bootstrap Web lineage differs from finalized images")
    expected = sorted({row["image"] for row in bootstrap["retained"]})
    require([row["image"] for row in bootstrap["retained"]] == expected,
            "Bootstrap Web lineage is ambiguous")
    require(isinstance(evidence, dict) and set(evidence) == {"schema", "deployments"}
            and evidence["schema"] == BOOTSTRAP_EVIDENCE_SCHEMA and isinstance(evidence["deployments"], list)
            and sha256(evidence) == bootstrap["approved_deployments_sha256"],
            "Bootstrap Web deployment evidence differs from its approval")
    proved = []
    identity_fields = {"image", "source_sha", "version"}
    hash_fields = {"publication_receipt_sha256", "controller_result_sha256"}
    for row in evidence["deployments"]:
        require(isinstance(row, dict) and set(row) == identity_fields | hash_fields | {"run_id", "run_attempt"}
                and immutable_image(row["image"])
                and all(type(row[key]) is int and row[key] > 0 for key in ("run_id", "run_attempt"))
                and all(matches(HASH, row[key]) for key in hash_fields),
                "Bootstrap Web deployment evidence row is invalid")
        proved.append({key: row[key] for key in identity_fields})
    scope = [*bootstrap["retained"], {key: incumbent[key] for key in identity_fields}]
    require(proved == sorted(scope, key=lambda value: value["image"]),
            "Bootstrap Web deployment evidence does not cover the retained and incumbent images")
    return [dict(row, completed_rollouts=0) for row in bootstrap["retained"]]


def production_inputs(finalized, serving_images, retain_rollouts):
    """Preserve the immutable catalog of each approved production exposure."""
    require(isinstance(serving_images, list) and serving_images
            and all(immutable_image(image) for image in serving_images)
            and serving_images == sorted(set(serving_images)),
            "Production Web continuity images are invalid")
    protected = {}
    for image in serving_images:
        matches_serving = [
            receipt for receipt in finalized
            if image == receipt["repository"] + "@" + receipt["digest"]
            or image == receipt.get("asset_continuity", {}).get("prepare_image")
        ]
        require(len(matches_serving) == 1, "Production Web image has no unique finalized lineage")
        [receipt] = matches_serving
        proof = receipt.get("asset_proof")
        inputs = proof["inputs"] if proof else [{
            "image": image, "source_sha": receipt["source"], "version": receipt["identity"]["version"],
        }]
        for row in inputs:
            row = dict(row, completed_rollouts=retain_rollouts + 1)
            prior = protected.get(row["image"])
            if prior is None:
                protected[row["image"]] = row
            else:
                require(all(prior[key] == row[key] for key in ("source_sha", "version"))
                        and ("catalog_sha256" not in prior or "catalog_sha256" not in row
                             or prior["catalog_sha256"] == row["catalog_sha256"]),
                        "Production Web catalog lineage conflicts")
                protected[row["image"]] = prior | row
    return protected


def retention_scope(preimage, started_at, finalized, *, bootstrap=None, bootstrap_evidence=None,
                    retain_rollouts=DEFAULT_RETENTION_ROLLOUTS, production_images=None):
    """Bind a scoped controller preimage to the lineage of its finalized serving image."""
    require(type(retain_rollouts) is int and retain_rollouts > 0,
            "Web asset retention needs a positive rollout count")
    require(isinstance(preimage, dict) and set(preimage) == {"schema", "observed_at", "incumbent", "asset_manifest_sha256"}
            and preimage["schema"] == PREIMAGE_SCHEMA and preimage["observed_at"] == started_at,
            "Web controller preimage differs from its admission")
    elapsed(started_at)
    incumbent = validate_incumbent(preimage["incumbent"])
    observed_manifest = preimage["asset_manifest_sha256"]
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
        require(observed_manifest == (continuity["asset_manifest_sha256"] if continuity else None),
                "Serving Web manifest differs from its finalized lineage")
        matches_serving.append((receipt.get("asset_proof"), promote == incumbent["image"]))
    require(len(matches_serving) == 1, "Serving Web image has no unique finalized lineage")
    protected = (production_inputs(finalized, production_images, retain_rollouts)
                 if production_images is not None else {})
    [(proof, promoted)] = matches_serving
    if proof is not None:
        inputs = [dict(row) for row in proof["inputs"]]
        if promoted:
            # Only a settled candidate counts as a new rollout. A prepare
            # image may be a rollback after the candidate served HTML.
            for row in inputs:
                row["completed_rollouts"] = (0 if row["image"] == incumbent["base_image"]
                                             else row["completed_rollouts"] + 1)
            if protected:
                inputs = [row for row in inputs if row["completed_rollouts"] <= retain_rollouts
                          or row["image"] in protected]
        if not protected:
            return {"incumbent": dict(incumbent), "preimage_sha256": sha256(preimage), "inputs": inputs}
        observation = preimage
    else:
        inputs = bootstrap_inputs(bootstrap, bootstrap_evidence, incumbent, finalized)
        observation = {"controller": preimage, "bootstrap": bootstrap}
    retained = {row["image"]: row for row in inputs}
    for image, row in protected.items():
        known = retained.get(image, {})
        require(all(known.get(key, row[key]) == row[key] for key in ("source_sha", "version"))
                and ("catalog_sha256" not in known or "catalog_sha256" not in row
                     or known["catalog_sha256"] == row["catalog_sha256"]),
                "Production and staging Web catalog lineage conflicts")
        retained[image] = row | known
    preimage_hash = sha256({"staging": observation, "production_images": production_images}) if protected else sha256(observation)
    return {"incumbent": dict(incumbent), "preimage_sha256": preimage_hash,
            "inputs": [retained[image] for image in sorted(retained)]}
