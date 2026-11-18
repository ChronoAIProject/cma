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
from staging_cd_publication_diagnostics import PublicationDiagnostics, run_publication
from staging_cd_receipts import canonical, published_receipt, validate_receipt
from staging_cd_registry import Registry
from staging_cd_reservations import validate_reservation
from staging_cd_versions import DEPLOYABLES


def publish(archive, identity, directory, registry, credential, actor, *, scanner=scan_staging_archive,
            runner=command, qualified=False, diagnostics=None):
    diagnostics = diagnostics or PublicationDiagnostics()
    repository = "ghcr.io/chronoaiproject/" + DEPLOYABLES[identity["component"]]
    diagnostics.mark("registry_preflight")
    require(registry.manifest(repository, identity["version"], absent=True) is None,
            "Reserved registry version already exists; publication refuses overwrite")
    diagnostics.mark("inspect")
    digest = inspect(archive, identity, runner, allow_qualified=qualified)
    report_dir = directory / "scan"
    diagnostics.mark("scan")
    scanned = scanner(archive, report_dir)
    report = json.loads((report_dir / "REPORT.json").read_text())
    diagnostics.mark("archive_check")
    require(checksum(archive) == scanned, "Scanned archive changed before publication")
    require(isinstance(credential, str) and credential and isinstance(actor, str) and actor,
            "Registry publisher credential is unavailable")
    auth = directory / "registry-auth.json"
    try:
        diagnostics.mark("registry_login")
        runner(["skopeo", "login", "--authfile", str(auth), "--username", actor, "--password-stdin", "ghcr.io"],
               data=credential.encode(), timeout=60)
        auth.chmod(0o600)
        diagnostics.mark("registry_copy")
        runner(["skopeo", "copy", "--preserve-digests", "--authfile", str(auth),
                "oci-archive:" + str(archive), "docker://" + repository + ":" + identity["version"]], timeout=1800)
    finally:
        auth.unlink(missing_ok=True)
    diagnostics.mark("archive_check")
    require(checksum(archive) == scanned, "Scanned archive changed during publication")
    diagnostics.mark("registry_verify")
    remote = registry.manifest(repository, identity["version"])
    require(remote is not None and remote[0] == digest and registry.exists(repository, digest),
            "Registry digest differs from the scanned archive")
    diagnostics.mark("receipt")
    return published_receipt(identity, digest, scanned, report["counts"])


def main():
    env, root = os.environ, Path.cwd()
    def operation(_masker, diagnostics):
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
            diagnostics.mark("validate")
            require(task["component"] == env["CD_COMPONENT"], "Publication component differs")
            if task["mode"] == "resume":
                diagnostics.mark("resume")
                receipt = validate_receipt(task["receipt"])
                require(registry.exists(receipt["repository"], receipt["digest"]), "Published digest is unavailable")
            else:
                require(task["mode"] == "publish", "Unknown publication task")
                reservation = validate_reservation(task["reservation"])
                require(reservation["run_id"] == int(env["GITHUB_RUN_ID"])
                        and reservation["run_attempt"] == int(env["GITHUB_RUN_ATTEMPT"]), "Reservation owner differs")
                with tempfile.TemporaryDirectory(prefix="cma-publication-", dir=env["RUNNER_TEMP"]) as temp:
                    directory = Path(temp)
                    source = directory / "source"
                    diagnostics.mark("snapshot")
                    snapshot(root, reservation["source"], source)
                    diagnostics.mark("validate")
                    release, _, _ = identity(reservation, source)
                    diagnostics.mark("reuse")
                    archive = try_reuse(release, directory, api, env.get("GH_TOKEN"))
                    qualified = archive is not None
                    if archive is None:
                        diagnostics.mark("build")
                        archive, release = build(root, reservation, directory, source=source)
                    receipt = publish(archive, release, directory, registry, env.get("GH_TOKEN"),
                                      env.get("GITHUB_ACTOR"), qualified=qualified, diagnostics=diagnostics)
            receipts.append(receipt)
            # Only fixed-schema source/image/scan counts cross the job boundary.
            # The raw scanner evidence and all image bytes are deleted locally.
            diagnostics.mark("receipt")
            output.write_text(canonical(receipts) + "\n", encoding="utf-8")
        return True
    return run_publication(operation)


if __name__ == "__main__":
    raise SystemExit(main())
