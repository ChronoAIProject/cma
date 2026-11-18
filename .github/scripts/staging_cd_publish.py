"""Publish exactly the bytes scanned by Policy A and emit only public receipts."""
import json
import os
import sys
import tempfile
from pathlib import Path

from image_secret_gate import checksum, scan_staging_archive
from staging_cd_admission import event_identity
from staging_cd_artifacts import try_reuse
from staging_cd_build import build, command, identity, inspect, snapshot
from staging_cd_common import require
from staging_cd_github import GitHub
from staging_cd_receipts import canonical, published_receipt, validate_receipt
from staging_cd_registry import Registry
from staging_cd_reservations import validate_reservation
from staging_cd_publication_diagnostics import PublicationDiagnostics, run_publication
from staging_cd_versions import DEPLOYABLES
from staging_cd_timing import Timings
from staging_cd_build_cache import BuildCache
from staging_cd_runtime_smoke import CleanupIncomplete, qualify as qualify_runtime
from staging_cd_targets import RUNTIME_TARGETS


def publish(archive, identity, directory, registry, credential, actor, *, scanner=scan_staging_archive,
            runner=command, qualified=False, smoke=qualify_runtime, source=None, diagnostics=None, timings=None):
    timings = timings or Timings()
    diagnostics = diagnostics or PublicationDiagnostics()
    repository = "ghcr.io/chronoaiproject/" + DEPLOYABLES[identity["component"]]
    diagnostics.mark("registry_preflight")
    with timings.stage("registry-check"):
        require(registry.manifest(repository, identity["version"], absent=True) is None,
                "Reserved registry version already exists; publication refuses overwrite")
    diagnostics.mark("inspect")
    with timings.stage("inspect"):
        digest = inspect(archive, identity, runner, allow_qualified=qualified)
    report_dir = directory / "scan"
    diagnostics.mark("scan")
    with timings.stage("scan"):
        scanned = scanner(archive, report_dir)
        report = json.loads((report_dir / "REPORT.json").read_text())
        diagnostics.mark("archive_check")
        require(checksum(archive) == scanned, "Scanned archive changed before publication")
    require(isinstance(credential, str) and credential and isinstance(actor, str) and actor,
            "Registry publisher credential is unavailable")
    if identity["component"] in RUNTIME_TARGETS:
        diagnostics.mark("runtime_smoke")
    with timings.stage("runtime-smoke"):
        proof = smoke(archive, identity, digest, scanned, directory, runner, source=source)
    diagnostics.mark("archive_check")
    require(checksum(archive) == scanned, "Qualified archive changed before publication")
    require(identity["component"] not in RUNTIME_TARGETS or proof is not None,
            "Runtime publication requires image smoke")
    diagnostics.mark("receipt")
    receipt = published_receipt(identity, digest, scanned, report["counts"])
    if proof is not None:
        receipt["runtime_smoke"] = proof
    validate_receipt(receipt)
    auth = directory / "registry-auth.json"
    try:
        diagnostics.mark("registry_login")
        with timings.stage("login"):
            runner(["skopeo", "login", "--authfile", str(auth), "--username", actor, "--password-stdin", "ghcr.io"],
                   data=credential.encode(), timeout=60)
            auth.chmod(0o600)
        diagnostics.mark("registry_copy")
        with timings.stage("push"):
            runner(["skopeo", "copy", "--preserve-digests", "--authfile", str(auth),
                    "oci-archive:" + str(archive), "docker://" + repository + ":" + identity["version"]], timeout=1800)
    finally:
        auth.unlink(missing_ok=True)
    diagnostics.mark("archive_check")
    with timings.stage("verify"):
        require(checksum(archive) == scanned, "Scanned archive changed during publication")
        diagnostics.mark("registry_verify")
        remote = registry.manifest(repository, identity["version"])
        require(remote is not None and remote[0] == digest and registry.exists(repository, digest),
                "Registry digest differs from the scanned archive")
    diagnostics.mark("receipt")
    return receipt


def main():
    env, root = os.environ, Path.cwd()
    timings = Timings()
    build_metrics = []
    cache_metrics = []
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
            cache = None
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
                    with timings.stage("source"):
                        snapshot(root, reservation["source"], source)
                        diagnostics.mark("validate")
                        release, _, _ = identity(reservation, source)
                    diagnostics.mark("reuse")
                    with timings.stage("reuse"):
                        archive = try_reuse(release, directory, api, env.get("GH_TOKEN"))
                    qualified = archive is not None
                    if archive is None:
                        if env.get("CD_BUILD_CACHE") == "enabled":
                            diagnostics.mark("cache_restore")
                            cache = BuildCache(root, env, source, reservation["component"])
                            with timings.stage("cache-restore"):
                                restored = cache.restore()
                            cache_metrics.append({"operation": "restore", "result": "hit" if restored else "miss"})
                        diagnostics.mark("build")
                        with timings.stage("build"):
                            progress = {}
                            build_metrics.append(progress)
                            archive, release = build(root, reservation, directory, source=source, cache=cache,
                                                     progress=progress)
                    try:
                        receipt = publish(archive, release, directory, registry, env.get("GH_TOKEN"),
                                          env.get("GITHUB_ACTOR"), qualified=qualified, source=source,
                                          diagnostics=diagnostics, timings=timings)
                    except CleanupIncomplete as error:
                        # Preserve only fixed-schema cleanup status and the
                        # synthetic campaign ID, never raw helper/daemon output.
                        output.with_name("runtime-cleanup.json").write_text(
                            canonical(error.receipt) + "\n", encoding="utf-8")
                        raise
            receipts.append(receipt)
            # Only fixed-schema source/image/scan counts cross the job boundary.
            # The raw scanner evidence and all image bytes are deleted locally.
            diagnostics.mark("receipt")
            output.write_text(canonical(receipts) + "\n", encoding="utf-8")
            # Persist successful publication before optional cache work. A later
            # cache timeout must not lose the receipt for an immutable pushed tag.
            if cache is not None:
                diagnostics.mark("cache_qualify")
                with timings.stage("cache-qualify"):
                    accepted = cache.qualify()
                cache_metrics.append({"operation": "qualify", "result": "accepted" if accepted else "withheld"})
                with Path(env["GITHUB_OUTPUT"]).open("a") as status:
                    status.write("build-cache-ready=" + ("true" if accepted else "false") + "\n")
        return True
    result = run_publication(operation)
    # run_private has restored the public descriptors; only fixed stage records
    # cross that boundary, including the failing phase on an unsuccessful run.
    if env.get("CD_COMPONENT") in ("am", "eg", "web"):
        timings.emit(sys.stdout)
        for metrics in build_metrics:
            value = metrics or {"complete": False, "reason": "progress-unavailable"}
            print(json.dumps({"event": "publication-build", **value}, sort_keys=True), flush=True)
        for metrics in cache_metrics:
            print(json.dumps({"event": "publication-cache", **metrics}, sort_keys=True), flush=True)
    return result


if __name__ == "__main__":
    raise SystemExit(main())
