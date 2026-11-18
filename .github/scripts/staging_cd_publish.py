"""Publish exactly the bytes scanned by Policy A and emit only public receipts."""
import json
import os
import tempfile
from pathlib import Path

from image_secret_gate import checksum, scan_staging_archive
from staging_cd_admission import event_identity
from staging_cd_artifacts import try_reuse
from staging_cd_build import build, command, identity, inspect, snapshot
from staging_cd_common import require
from staging_cd_github import GitHub
from staging_cd_progress import PublicationProgress, PublicationStage
from staging_cd_receipts import canonical, published_receipt, validate_receipt
from staging_cd_registry import Registry
from staging_cd_reservations import validate_reservation
from staging_cd_retirements import guard_retired
from staging_cd_runtime import run_private
from staging_cd_versions import DEPLOYABLES


def publish(archive, identity, directory, registry, credential, actor, *, scanner=scan_staging_archive,
            runner=command, qualified=False, progress=None):
    progress = progress or PublicationProgress()
    guard_retired(identity["component"], identity["source"])
    repository = "ghcr.io/chronoaiproject/" + DEPLOYABLES[identity["component"]]
    progress.enter(PublicationStage.ABSENCE)
    require(registry.manifest(repository, identity["version"], absent=True) is None,
            "Reserved registry version already exists; publication refuses overwrite")
    progress.enter(PublicationStage.INSPECT)
    digest = inspect(archive, identity, runner, allow_qualified=qualified)
    report_dir = directory / "scan"
    progress.enter(PublicationStage.SCAN)
    scanned = scanner(archive, report_dir, component=identity["component"])
    report = json.loads((report_dir / "REPORT.json").read_text())
    require(checksum(archive) == scanned, "Scanned archive changed before publication")
    progress.enter(PublicationStage.LOGIN)
    require(isinstance(credential, str) and credential and isinstance(actor, str) and actor,
            "Registry publisher credential is unavailable")
    auth = directory / "registry-auth.json"
    try:
        runner(["skopeo", "login", "--authfile", str(auth), "--username", actor, "--password-stdin", "ghcr.io"],
               data=credential.encode(), timeout=60)
        auth.chmod(0o600)
        progress.enter(PublicationStage.PUSH)
        runner(["skopeo", "copy", "--preserve-digests", "--authfile", str(auth),
                "oci-archive:" + str(archive), "docker://" + repository + ":" + identity["version"]], timeout=1800)
    finally:
        auth.unlink(missing_ok=True)
    progress.enter(PublicationStage.VERIFY)
    require(checksum(archive) == scanned, "Scanned archive changed during publication")
    remote = registry.manifest(repository, identity["version"])
    require(remote is not None and remote[0] == digest and registry.exists(repository, digest),
            "Registry digest differs from the scanned archive")
    progress.enter(PublicationStage.RECEIPT)
    return published_receipt(identity, digest, scanned, report["counts"])


def main():
    env, root = os.environ, Path.cwd()
    progress = PublicationProgress()
    def operation(_masker):
        event_identity(env)
        require(env["GITHUB_EVENT_NAME"] == "push", "Image publication only accepts develop pushes")
        tasks = json.loads(env["CD_TASKS"])
        require(isinstance(tasks, list) and tasks, "Image publication tasks are missing")
        receipts = []
        output = Path(env["CD_RECEIPTS_PATH"])
        output.parent.mkdir(parents=True, exist_ok=True)
        registry = Registry(credential=env.get("GH_TOKEN"), actor=env.get("GITHUB_ACTOR"))
        api = GitHub(env.get("GH_TOKEN"))
        for task in tasks:
            progress.enter(PublicationStage.INPUT)
            require(task["component"] == env["CD_COMPONENT"], "Publication component differs")
            if task["mode"] == "resume":
                progress.enter(PublicationStage.RESUME)
                receipt = validate_receipt(task["receipt"])
                guard_retired(receipt["identity"]["component"], receipt["source"], root)
                require(registry.exists(receipt["repository"], receipt["digest"]), "Published digest is unavailable")
            else:
                require(task["mode"] == "publish", "Unknown publication task")
                reservation = validate_reservation(task["reservation"])
                guard_retired(reservation["component"], reservation["source"], root)
                require(reservation["run_id"] == int(env["GITHUB_RUN_ID"])
                        and reservation["run_attempt"] == int(env["GITHUB_RUN_ATTEMPT"]), "Reservation owner differs")
                with tempfile.TemporaryDirectory(prefix="cma-publication-", dir=env["RUNNER_TEMP"]) as temp:
                    directory = Path(temp)
                    source = directory / "source"
                    progress.enter(PublicationStage.SNAPSHOT)
                    snapshot(root, reservation["source"], source)
                    progress.enter(PublicationStage.IDENTITY)
                    release, _, _ = identity(reservation, source)
                    progress.enter(PublicationStage.REUSE)
                    archive = try_reuse(release, directory, api, env.get("GH_TOKEN"))
                    qualified = archive is not None
                    if archive is None:
                        progress.enter(PublicationStage.BUILD)
                        archive, release = build(root, reservation, directory, source=source)
                    receipt = publish(archive, release, directory, registry, env.get("GH_TOKEN"),
                                      env.get("GITHUB_ACTOR"), qualified=qualified, progress=progress)
            receipts.append(receipt)
            # Only fixed-schema source/image/scan counts cross the job boundary.
            # The raw scanner evidence and all image bytes are deleted locally.
            progress.enter(PublicationStage.RECEIPT)
            output.write_text(canonical(receipts) + "\n", encoding="utf-8")
        return True
    return run_private(operation, None, publication_progress=progress)


if __name__ == "__main__":
    raise SystemExit(main())
