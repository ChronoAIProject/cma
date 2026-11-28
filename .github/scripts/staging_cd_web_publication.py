"""Publish a protected Web release inside the same writer epoch as its promotion."""

from pathlib import Path

import ci_retry
from image_secret_gate import checksum, verified_staging_scan
from staging_cd_build import command, inspect
from staging_cd_common import require
from staging_cd_publication_diagnostics import ImageCommandRefusal, skopeo_version
from staging_cd_publish import push
from staging_cd_receipts import published_receipt, validate_receipt
from staging_cd_web_continuity import REPOSITORY, immutable_image, require_images
from staging_cd_web_overlay import WebArchive, assemble, qualify


def archive_base(row, directory, auth, runner):
    image = row["image"]
    require(immutable_image(image), "Web retention input must be an immutable registry image")
    digest = image.split("@")[1]
    archive = directory / ("base-" + digest[7:] + ".oci.tar")
    require(not archive.exists() and not archive.is_symlink(), "Web asset archive output already exists")
    runner(["skopeo", "copy", "--preserve-digests", "--src-authfile", str(auth),
            "docker://" + image, "oci-archive:" + str(archive)], timeout=300)
    return WebArchive(archive, digest, row["source_sha"], row["version"], row.get("catalog_sha256"),
                      row.get("completed_rollouts", 0))


def protected_pair(archive, digest, identity, scope, directory, source, auth, runner, *, max_files, max_bytes):
    incumbent = scope["incumbent"]
    base_ref = incumbent["base_image"]
    inputs = {item["image"]: item for item in scope["inputs"]}
    require(len(inputs) == len(scope["inputs"]), "Web retention input is duplicated")
    live_base = inputs.pop(base_ref, {"image": base_ref, "source_sha": incumbent["source_sha"],
                                    "version": incumbent["version"]})
    require((live_base["source_sha"], live_base["version"]) == (incumbent["source_sha"], incumbent["version"]),
            "Web retention incumbent base differs from the controller")
    old = archive_base(dict(live_base, completed_rollouts=0), directory, auth, runner)
    retained = [archive_base(row, directory, auth, runner) for row in inputs.values()]
    candidate = WebArchive(archive, digest, identity["source"], identity["version"])
    pair = assemble(old, candidate, retained, directory, max_files=max_files, max_bytes=max_bytes)
    evidence = qualify(pair, incumbent, identity, digest, scope["preimage_sha256"], source)
    return pair, evidence


def copy_auxiliary(store, registry, auth, runner, reservation_check, retry):
    archive, digest, scanned = store
    reference = "asset-sha256-" + digest[7:]
    require(checksum(archive) == scanned, "Web auxiliary archive differs from its scan")
    current = registry.manifest(REPOSITORY, reference, absent=True)
    if current is None:
        push(archive, REPOSITORY, reference, digest, auth, registry, runner, reservation_check, retry)
    else:
        require(current[0] == digest, "Web auxiliary tag belongs to a different digest")
    confirmed = registry.manifest(REPOSITORY, reference, settle=True)
    require(confirmed is not None and confirmed[0] == digest and checksum(archive) == scanned,
            "Web auxiliary archive publication was not verified")


def publish(archive, identity, scope, directory, registry, credential, actor, *, source, reservation_check,
            prepared_checksum, max_files, max_bytes, runner=command, retry=None, mark=lambda _stage: None):
    """Require the exact raw archive and scoped preimage before any registry write."""
    require(identity["component"] == "web" and reservation_check() is True,
            "Web continuity publication is not current")
    require(checksum(archive) == prepared_checksum, "Raw Web archive differs from its producer")
    mark("inspect")
    digest = inspect(archive, identity, runner)
    version = identity["version"]
    mark("registry_preflight")
    require(registry.manifest(REPOSITORY, version, absent=True) is None,
            "Reserved registry version already exists; publication refuses overwrite")
    mark("scan")
    prepared, _ = verified_staging_scan(archive, directory / "base-scan", component="web", source=source)
    require(prepared == prepared_checksum and checksum(archive) == prepared,
            "Qualified Web base archive differs from its producer")
    require(isinstance(credential, str) and credential and isinstance(actor, str) and actor,
            "Registry publisher credential is unavailable")
    retry = dict(retry or {})
    auth = Path(directory) / "web-registry-auth.json"
    try:
        mark("registry_login")
        ci_retry.call(lambda: runner(["skopeo", "login", "--authfile", str(auth), "--username", actor,
                                      "--password-stdin", "ghcr.io"], data=credential.encode(), timeout=60),
                      name="registry-login", classify=ci_retry.classify, **retry)
        auth.chmod(0o600)
        mark("registry_preflight")
        pair, evidence = protected_pair(archive, digest, identity, scope, directory, source, auth, runner,
                                        max_files=max_files, max_bytes=max_bytes)
        promote = pair["promote"]
        counts = evidence.pop("counts")
        receipt = published_receipt(identity, promote["digest"], promote["archive_sha256"], counts, **evidence)
        prepare = pair["prepare"]
        mark("registry_copy")
        role = "raw"
        try:
            copy_auxiliary((archive, digest, prepared), registry, auth, runner, reservation_check, retry)
            role = "prepare"
            copy_auxiliary((prepare["archive"], prepare["digest"], prepare["archive_sha256"]),
                           registry, auth, runner, reservation_check, retry)
            role = "promote"
            require(checksum(promote["archive"]) == receipt["archive"], "Promote Web archive differs from its scan")
            push(promote["archive"], REPOSITORY, version, receipt["digest"], auth, registry, runner, reservation_check, retry)
        except ImageCommandRefusal as error:
            if error.copy_failure is not None:
                error.web_copy_failure = (role, *error.copy_failure, skopeo_version(runner))
            raise
    finally:
        auth.unlink(missing_ok=True)
    require(checksum(promote["archive"]) == receipt["archive"], "Promote Web archive changed during publication")
    mark("registry_verify")
    remote = registry.manifest(REPOSITORY, version, settle=True)
    require(remote is not None and remote[0] == receipt["digest"]
            and registry.exists(REPOSITORY, receipt["digest"], settle=True), "Registry digest differs from the Web archive")
    require(reservation_check() is True, "Publication reservation is not current")
    return receipt


def resume(receipt, scope, registry):
    """Recover a finalized Web deployment without writing its immutable images again."""
    validate_receipt(receipt)
    continuity = receipt.get("asset_continuity")
    require(continuity is not None, "Protected Web publication cannot adopt a legacy receipt")
    incumbent = continuity["incumbent"]
    promote = receipt["repository"] + "@" + receipt["digest"]
    prepared = dict(incumbent, image=continuity["prepare_image"])
    promoted = {"image": promote, "base_image": continuity["candidate_base_image"],
                "source_sha": receipt["source"], "version": receipt["identity"]["version"]}
    require(scope["incumbent"] in (incumbent, prepared, promoted),
            "Finalized Web release has a stale controller preimage")
    remote = registry.manifest(REPOSITORY, receipt["identity"]["version"])
    require(remote is not None and remote[0] == receipt["digest"] and registry.exists(REPOSITORY, receipt["digest"]),
            "Finalized Web release differs from its reserved version")
    require_images(continuity, receipt["asset_proof"], registry)
    return receipt
