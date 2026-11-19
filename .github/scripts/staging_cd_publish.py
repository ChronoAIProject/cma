"""Publish exactly the bytes scanned by Policy A and emit only public receipts."""
import json
import os
import sys
import tempfile
from pathlib import Path

from image_secret_gate import ScanRefusal, checksum, verified_staging_scan
from staging_cd_admission import event_identity
from staging_cd_artifacts import try_reuse
from staging_cd_build import build, command, identity, inspect, snapshot
from staging_cd_common import matches, require
from staging_cd_github import GitHub
from staging_cd_release_tags import local_tag_reader
from staging_cd_resolve import output as step_output
from staging_cd_receipts import canonical, published_receipt, validate_receipt
from staging_cd_registry import Registry
from staging_cd_reservations import Reservations, validate_reservation
from staging_cd_publication_diagnostics import PublicationDiagnostics, run_publication
from staging_cd_versions import DEPLOYABLES
from staging_cd_timing import Timings
from staging_cd_build_cache import BuildCache
from staging_cd_runtime_smoke import CleanupIncomplete, qualify as qualify_runtime
from staging_cd_targets import APP_TARGETS, RUNTIME_TARGETS, TARGETS
from staging_cd_contract import GROUP, from_source


def validated_tasks(tasks, env):
    require(isinstance(tasks, list) and tasks, "Image publication tasks are missing")
    component = env["CD_COMPONENT"]
    validated = []
    # Check the whole component queue before acting on its first reservation.
    for task in tasks:
        require(isinstance(task, dict) and task.get("component") == component, "Publication component differs")
        mode = task.get("mode")
        if mode == "resume":
            value = validate_receipt(task.get("receipt"))
            require(value["identity"]["component"] == component, "Resumed receipt component differs")
        else:
            require(mode == "publish", "Unknown publication task")
            value = validate_reservation(task.get("reservation"))
            require(value["component"] == component, "Reservation component differs")
            require(value["run_id"] == int(env["GITHUB_RUN_ID"])
                    and value["run_attempt"] == int(env["GITHUB_RUN_ATTEMPT"]), "Reservation owner differs")
        validated.append((mode, value))
    return validated


def retain_package_evidence(report, directory, source, image_digest):
    if "package_evidence" not in report:
        return
    require(directory is not None, "Verified package evidence needs a retention path")
    require(matches(r"[a-f0-9]{64}", report.get("archive_sha256")),
            "Verified package evidence needs the exact archive digest")
    # The gate has already validated these redacted findings and classifications.
    fields = ("archive_sha256", "package_evidence", "findings", "reviewed_classifications",
              "blocking_findings", "advisory_findings", "counts")
    payload = {key: report[key] for key in fields}
    payload.update(schema="cma-staging-package-scan-v1", source=source, image_digest=image_digest,
                   evidence_sha256=report["policy"]["package_evidence_sha256"])
    directory.mkdir(parents=True, exist_ok=True)
    target = directory / (report["archive_sha256"] + ".json")
    with target.open("x", encoding="utf-8") as stream:
        stream.write(canonical(payload) + "\n")


def publish(archive, identity, directory, registry, credential, actor, *, scanner=verified_staging_scan,
            runner=command, qualified=False, smoke=qualify_runtime, source=None, diagnostics=None, timings=None,
            reservation_check, scan_evidence=None):
    require(reservation_check() is True, "Publication reservation is not current")
    timings = timings or Timings()
    diagnostics = diagnostics or PublicationDiagnostics()
    require(identity["component"] not in GROUP or source is not None,
            "AM/runtime publication requires source-derived compatibility")
    compatibility = from_source(identity["component"], identity["source"], source)
    repository = "ghcr.io/chronoaiproject/" + DEPLOYABLES[identity["component"]]
    diagnostics.mark("registry_preflight")
    with timings.stage("registry-check"):
        require(registry.manifest(repository, identity["version"], absent=True) is None,
                "Reserved registry version already exists; publication refuses overwrite")
    diagnostics.mark("inspect")
    with timings.stage("inspect"):
        digest = inspect(archive, identity, runner, allow_qualified=qualified, compatibility=compatibility)
    report_dir = directory / "scan"
    diagnostics.mark("scan")
    with timings.stage("scan"):
        try:
            scanned, scanner_identity = scanner(archive, report_dir, component=identity["component"])
        except ScanRefusal as error:
            if error.package_report is not None:
                retain_package_evidence(error.package_report, scan_evidence, identity["source"], digest)
            raise
        report = json.loads((report_dir / "REPORT.json").read_text())
        runtime_scope = identity["component"] if identity["component"] in RUNTIME_TARGETS else None
        require(isinstance(report.get("policy"), dict) and report["policy"].get("staging_runtime_component") == runtime_scope,
                "Verified scan component differs from the release")
        timings.detector_cache(report.get("detector_cache"))
        diagnostics.mark("archive_check")
        require(checksum(archive) == scanned, "Scanned archive changed before publication")
        retain_package_evidence(report, scan_evidence, identity["source"], digest)
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
    receipt = published_receipt(identity, digest, scanned, report["counts"], scanner_identity=scanner_identity,
                                compatibility=compatibility)
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
        # Qualification can take minutes; re-read authority at the write boundary.
        require(reservation_check() is True, "Publication reservation is not current")
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
    # A copy already in flight may finish writing its own burned version.
    # It cannot hand a receipt to finalization after a successor takes over.
    require(reservation_check() is True, "Publication reservation is not current")
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
        diagnostics.mark("validate")
        tasks = validated_tasks(json.loads(env["CD_TASKS"]), env)
        receipts = []
        output = Path(env["CD_RECEIPTS_PATH"])
        output.parent.mkdir(parents=True, exist_ok=True)
        registry = Registry(credential=env.get("GH_TOKEN"), actor=env.get("GITHUB_ACTOR"))
        api = GitHub(env.get("GH_TOKEN"), local_tags=local_tag_reader(env.get("CD_COMPONENT"), Path.cwd()))
        for mode, value in tasks:
            cache = None
            diagnostics.mark("validate")
            if mode == "resume":
                diagnostics.mark("resume")
                receipt = value
                require(registry.exists(receipt["repository"], receipt["digest"]), "Published digest is unavailable")
            else:
                reservation = value
                reservations = Reservations(api, env.get("GH_TOKEN"), root)
                reservations.require_current(reservation)
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
                                cache.restore()
                            cache_metrics.append({"operation": "restore", "result": cache.restore_result})
                        diagnostics.mark("build")
                        with timings.stage("build"):
                            progress: dict = {}
                            build_metrics.append(progress)
                            guard = {}
                            if reservation["component"] == "am":
                                def check_owner(probe):
                                    credential = env.get("GH_TOKEN")
                                    lookup = GitHub(credential, runner=probe)
                                    Reservations(lookup, credential, root).check_build_owner(reservation)
                                guard["check_owner"] = check_owner
                            archive, release = build(root, reservation, directory, source=source, cache=cache,
                                                     progress=progress, **guard)
                    require(release["component"] == reservation["component"], "Built release component differs")
                    try:
                        receipt = publish(archive, release, directory, registry, env.get("GH_TOKEN"),
                                          env.get("GITHUB_ACTOR"), qualified=qualified, source=source,
                                          diagnostics=diagnostics, timings=timings,
                                          scan_evidence=output.parent / "package-policy",
                                          reservation_check=lambda: reservations.require_current(reservation))
                    except CleanupIncomplete as error:
                        # Preserve only fixed-schema cleanup status and the
                        # synthetic campaign ID, never raw helper/daemon output.
                        output.with_name("runtime-cleanup.json").write_text(
                            canonical(error.receipt) + "\n", encoding="utf-8")
                        raise
            receipts.append(receipt)
            # Publication receipts keep their existing schema; verified package
            # decisions are retained separately from raw scan directory and image bytes.
            diagnostics.mark("receipt")
            output.write_text(canonical(receipts) + "\n", encoding="utf-8")
            # Persist successful publication before optional cache work. A later
            # cache timeout must not lose the receipt for an immutable pushed tag.
            if cache is not None and cache.restore_result != "hit":
                diagnostics.mark("cache_export")
                with timings.stage("cache-export"):
                    accepted = cache.export_mounts()
                cache_metrics.append({"operation": "export", "result": "accepted" if accepted else "withheld"})
                with Path(env["GITHUB_OUTPUT"]).open("a") as status:
                    if accepted:
                        status.write("build-cache-key=" + cache.key.archive_key(env) + "\n")
                    status.write("build-cache-ready=" + ("true" if accepted else "false") + "\n")
        return True
    def superseded():
        require(env.get("CD_COMPONENT") == "am", "Unexpected superseded component")
        step_output(env, "disposition", "reservation_superseded")

    result = run_publication(operation, on_superseded=superseded)
    # run_private has restored the public descriptors; only fixed stage records
    # cross that boundary, including the failing phase on an unsuccessful run.
    component = env.get("CD_COMPONENT")
    if component in TARGETS:
        timings.emit(sys.stdout, scan_only=component in RUNTIME_TARGETS)
    if component in APP_TARGETS:
        for metrics in build_metrics:
            value = metrics or {"complete": False, "reason": "progress-unavailable"}
            print(json.dumps({"event": "publication-build", **value}, sort_keys=True), flush=True)
        for metrics in cache_metrics:
            print(json.dumps({"event": "publication-cache", **metrics}, sort_keys=True), flush=True)
    return result


if __name__ == "__main__":
    raise SystemExit(main())
