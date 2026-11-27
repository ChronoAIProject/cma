"""Publish exactly the bytes scanned by Policy A and emit only public receipts."""
import json
import os
import sys
import tempfile
import time
from contextlib import ExitStack
from pathlib import Path

import ci_retry
from ci_outcome import report
from image_secret_gate import ScanRefusal, checksum, verified_staging_scan
from staging_cd_admission import event_identity
from staging_cd_artifacts import try_reuse
from staging_cd_build import build, command, identity, inspect, snapshot
from staging_cd_common import Refusal, matches, require
from staging_cd_cargo_artifact import artifact_name as cargo_artifact_name, index_name as cargo_index_name
from staging_cd_github import GitHub
from staging_cd_native_archive import COMPONENTS as NATIVE_COMPONENTS, try_fetch as fetch_native
from staging_cd_release_archive import load as load_prepared_archive, path_for
from staging_cd_release_tags import local_tag_reader
from staging_cd_resolve import output as step_output
from staging_cd_receipts import canonical, published_receipt, validate_receipt
from staging_cd_registry import Registry
from staging_cd_reservations import Reservations, SourceAlreadyFinal, validate_reservation
from staging_cd_publication_diagnostics import PublicationDiagnostics, run_publication
from staging_cd_versions import DEPLOYABLES, require_publishable_release
from staging_cd_timing import Timings
from staging_cd_build_cache import SCOPES, BuildCache
from staging_cd_cache_save import report_layers
from staging_cd_runtime_smoke import CleanupIncomplete, qualify as qualify_runtime
from staging_cd_targets import APP_TARGETS, RUNTIME_TARGETS, TARGETS
from staging_cd_test_failures import annotate as annotate_failed_tests, public_summary
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
        require_publishable_release(value["identity"] if mode == "resume" else value)
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


# Every copy attempt shares the original single copy's bound, so a repeat never
# extends the publish step beyond what one copy could already take.
COPY_SECONDS = 1800


def push(archive, repository, version, digest, auth, registry, runner, reservation_check, retry):
    """Copy the scanned bytes to their reserved, immutable tag.

    A transport failure leaves the outcome uncertain. The tag is read back
    first: our digest means the copy landed; another digest refuses; only a
    proven absence permits one repeat, after authority is re-read, inside the
    copy budget.
    """
    clock = retry.get("clock", time.monotonic)
    deadline = clock() + COPY_SECONDS

    def copy():
        # Qualification can take minutes; re-read authority at each write boundary.
        require(reservation_check() is True, "Publication reservation is not current")
        timeout = ci_retry.attempt_timeout(COPY_SECONDS, deadline, clock)
        require(timeout is not None, "Registry copy budget expired")
        runner(["skopeo", "copy", "--preserve-digests", "--authfile", str(auth),
                "oci-archive:" + str(archive), "docker://" + repository + ":" + version], timeout=timeout)

    def readback():
        remote = registry.manifest(repository, version, absent=True)
        if remote is None:
            return ci_retry.ABSENT
        return ci_retry.COMMITTED if remote[0] == digest else ci_retry.CONFLICT

    try:
        # Registry reads already repeat transient failures, so the readback runs once per settlement.
        ci_retry.call(lambda: ci_retry.call_write(copy, readback, name="registry-copy", classify=ci_retry.classify,
                                                  readback_attempts=1, **retry),
                      name="registry-copy", classify=ci_retry.classify, attempts=2,
                      **ci_retry.bounded(retry, deadline))
    except ci_retry.WriteConflict as error:
        raise Refusal("Registry digest differs from the scanned archive") from error
    except ci_retry.WriteUncertain as error:
        raise Refusal("Registry publication outcome is uncertain") from error


def publish(archive, identity, directory, registry, credential, actor, *, scanner=None,
            runner=command, qualified=False, smoke=qualify_runtime, source=None, diagnostics=None, timings=None,
            reservation_check, scan_evidence=None, retry=None, prepared_checksum=None, on_verified_image=None):
    require_publishable_release(identity)
    require(reservation_check() is True, "Publication reservation is not current")
    retry = dict(retry or {})
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
            if scanner is None:
                scanned, scanner_identity = verified_staging_scan(
                    archive, report_dir, component=identity["component"], source=source)
            else:
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
        require(prepared_checksum is None or prepared_checksum == scanned,
                "Scanned archive differs from the producer archive")
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
    if on_verified_image is not None:
        on_verified_image()
    auth = directory / "registry-auth.json"
    try:
        diagnostics.mark("registry_login")
        with timings.stage("login"):
            # Login only authenticates this runner; repeating it after a transport failure is safe.
            ci_retry.call(lambda: runner(["skopeo", "login", "--authfile", str(auth), "--username", actor,
                                          "--password-stdin", "ghcr.io"], data=credential.encode(), timeout=60),
                          name="registry-login", classify=ci_retry.classify, **retry)
            auth.chmod(0o600)
        diagnostics.mark("registry_copy")
        with timings.stage("push"):
            push(archive, repository, identity["version"], digest, auth, registry, runner, reservation_check, retry)
    finally:
        auth.unlink(missing_ok=True)
    diagnostics.mark("archive_check")
    with timings.stage("verify"):
        require(checksum(archive) == scanned, "Scanned archive changed during publication")
        diagnostics.mark("registry_verify")
        # The copy just landed; read it back with the settle window, not the first edge answer.
        remote = registry.manifest(repository, identity["version"], settle=True)
        require(remote is not None and remote[0] == digest and registry.exists(repository, digest, settle=True),
                "Registry digest differs from the scanned archive")
    # A started build ignores a successor's pending reservation, but it cannot
    # hand a receipt to finalization after a higher final has landed.
    require(reservation_check() is True, "Publication reservation is not current")
    diagnostics.mark("receipt")
    return receipt


def emit_cache_metrics(component, cache_metrics, stream):
    for metrics in cache_metrics:
        print(json.dumps({"event": "publication-cache", **metrics}, sort_keys=True), file=stream, flush=True)
        if metrics.get("operation") != "export":
            continue
        if metrics.get("result") == "withheld" and isinstance(metrics.get("reason"), str):
            # A withheld bundle slows the next build; the catalogue says why and what to do.
            report("cache-withheld", metrics["reason"], subject="build-cache", component=component, stream=stream)
        elif metrics.get("headroom") == "low":
            # Fixed text only: surface the bound before builds quietly slow down.
            print(f"::warning title=Cargo cache bound::{component} Cargo bundle is "
                  "above 75% of its bound in staging_cd_cargo_artifact.py", file=stream, flush=True)


def record_cache_export(cache, accepted, env, metrics):
    outputs = {}
    if accepted:
        component = env["CD_COMPONENT"]
        outputs.update({
            "build-cache-directory": str(cache.accepted),
            "build-cache-key": cache.key.archive_key(env),
            "build-cache-artifact": cargo_artifact_name(component, cache.key.family),
            "build-cache-index": cargo_index_name(component, cache.key.family, cache.key.prefix),
        })
    outputs["build-cache-ready"] = "true" if accepted else "false"
    try:
        with Path(env["GITHUB_OUTPUT"]).open("a") as status:
            status.write("".join(f"{key}={value}\n" for key, value in outputs.items()))
    except (OSError, KeyError):
        accepted = False
        cache.export_reason = "invalid"

    metric = {"operation": "export", "result": "accepted" if accepted else "withheld"}
    if not accepted and isinstance(cache.export_reason, str):
        metric["reason"] = cache.export_reason
    if accepted and cache.export_headroom == "low":
        metric["headroom"] = "low"
    if accepted and cache.export_retained_groups is not None:
        metric["retained_groups"] = cache.export_retained_groups
    metrics.append(metric)


def main():
    env, root = os.environ, Path.cwd()
    timings = Timings()
    build_metrics = []
    cache_metrics = []
    reuse_misses = []
    reused_prepared = []
    reused_native = []
    layer_exports = []
    skip = {"disposition": "reservation_superseded"}
    def operation(_masker, diagnostics):
        event_identity(env)
        require(env["GITHUB_EVENT_NAME"] == "push", "Image publication only accepts develop pushes")
        diagnostics.mark("validate")
        tasks = validated_tasks(json.loads(env["CD_TASKS"]), env)
        native_artifact = env.get("CD_NATIVE_IMAGE_ARTIFACT")
        require(not native_artifact or (len(tasks) == 1 and tasks[0][0] == "publish"
                and env["CD_COMPONENT"] in NATIVE_COMPONENTS), "Native image handoff tasks differ")
        web_scope = env.get("CD_WEB_CONTINUITY_SCOPE")
        protected_web = env.get("CD_WEB_CONTINUITY") == "required"
        if protected_web:
            from staging_cd_set import document
            from staging_cd_web_continuity import validate_scope
            from staging_cd_web_publication import publish as publish_web, resume as resume_web
            require(env["CD_COMPONENT"] == "web" and len(tasks) == 1, "Protected Web publication requires one task")
            web_scope = validate_scope(document(web_scope))
        else:
            require(not web_scope, "Web publication scope requires the protected writer")
        receipts = []
        output = Path(env["CD_RECEIPTS_PATH"])
        output.parent.mkdir(parents=True, exist_ok=True)
        registry = Registry(credential=env.get("GH_TOKEN"), actor=env.get("GITHUB_ACTOR"))
        api = GitHub(env.get("GH_TOKEN"), local_tags=local_tag_reader(root))

        def retain(receipt):
            receipts.append(receipt)
            diagnostics.mark("receipt")
            output.write_text(canonical(receipts) + "\n", encoding="utf-8")

        for mode, value in tasks:
            cache = None
            diagnostics.mark("validate")
            if mode == "resume":
                diagnostics.mark("resume")
                receipt = resume_web(value, web_scope, registry) if web_scope else value
                require(registry.exists(receipt["repository"], receipt["digest"]), "Published digest is unavailable")
                retain(receipt)
            else:
                reservation = value
                reservations = Reservations(api, env.get("GH_TOKEN"), root)
                prepared_path = None
                if env.get("CD_PREPARED_ARCHIVE") == "true":
                    prepared_path = path_for(Path(env["RUNNER_TEMP"]) / "cma-prepared-release", reservation)
                require(not web_scope or prepared_path is not None, "Protected Web publication requires a prepared archive")
                # A queued release can yield before a fallback build. A producer
                # that already finished its build retains the same write fences.
                try:
                    reservations.require_current(reservation, build_started=prepared_path is not None or bool(native_artifact))
                except SourceAlreadyFinal:
                    # An older run published this source and no run is known to deploy
                    # this group, so the report fails this skip instead of handling it.
                    skip["disposition"] = "source_already_final"
                    raise
                with tempfile.TemporaryDirectory(prefix="cma-publication-", dir=env["RUNNER_TEMP"]) as temp:
                    directory = Path(temp)
                    source = directory / "source"
                    diagnostics.mark("snapshot")
                    with timings.stage("source"):
                        snapshot(root, reservation["source"], source)
                        diagnostics.mark("validate")
                        release, _, _ = identity(reservation, source)
                    diagnostics.mark("reuse")
                    qualified = False
                    prepared_checksum = None
                    with timings.stage("reuse"):
                        if native_artifact:
                            prepared = fetch_native(release, native_artifact, directory, source, api,
                                                    env.get("GH_TOKEN"), reuse_misses.append)
                            archive = prepared.path if prepared is not None else None
                            if prepared is not None:
                                prepared_checksum = prepared.sha256
                                reused_native.append(release["component"])
                            else:
                                reservations.require_current(reservation, build_started=False)
                        elif prepared_path is not None:
                            prepared = load_prepared_archive(prepared_path, release)
                            require(prepared is not None, "Prepared release archive is missing")
                            archive = prepared.path
                            prepared_checksum = prepared.sha256
                            reused_prepared.append(True)
                            if prepared.cache_day is not None:
                                cache = BuildCache(root, env, source, reservation["component"],
                                                   cache_day=prepared.cache_day)
                                cache.namespace = prepared.cache_namespace
                        else:
                            archive = try_reuse(release, directory, api, env.get("GH_TOKEN"),
                                                on_miss=reuse_misses.append, source_root=source)
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
                            started = time.time()
                            archive, release = build(root, reservation, directory, source=source, cache=cache,
                                                     progress=progress)
                        if cache is not None and cache.exports_layers() is True:
                            layer_exports.append((started, progress))
                    require(release["component"] == reservation["component"], "Built release component differs")
                    needs_cache = cache is not None and cache.key is not None
                    finish_export = None
                    with ExitStack() as cache_work:
                        def start_cache_export():
                            nonlocal finish_export
                            if needs_cache and cache.can_overlap_export(archive):
                                finish_export = cache_work.enter_context(cache.background_export(directory))

                        try:
                            def current():
                                return reservations.require_current(reservation, build_started=True)

                            if web_scope:
                                receipt = publish_web(archive, release, web_scope, directory, registry,
                                                      env.get("GH_TOKEN"), env.get("GITHUB_ACTOR"), source=source,
                                                      reservation_check=current, prepared_checksum=prepared_checksum,
                                                      max_files=int(env["CD_WEB_RETENTION_FILE_LIMIT"]),
                                                      max_bytes=int(env["CD_WEB_RETENTION_BYTE_LIMIT"]),
                                                      mark=diagnostics.mark)
                            else:
                                receipt = publish(archive, release, directory, registry, env.get("GH_TOKEN"),
                                                  env.get("GITHUB_ACTOR"), qualified=qualified, source=source,
                                                  diagnostics=diagnostics, timings=timings,
                                                  scan_evidence=output.parent / "package-policy",
                                                  prepared_checksum=prepared_checksum,
                                                  on_verified_image=start_cache_export, reservation_check=current)
                        except CleanupIncomplete as error:
                            # Keep only the fixed-schema cleanup receipt.
                            output.with_name("runtime-cleanup.json").write_text(
                                canonical(error.receipt) + "\n", encoding="utf-8")
                            raise
                        # The verified registry receipt precedes cache acceptance.
                        retain(receipt)
                        if finish_export is not None:
                            diagnostics.mark("cache_export")
                            with timings.stage("cache-export-wait"):
                                accepted = finish_export()
                            record_cache_export(cache, accepted, env, cache_metrics)
                if needs_cache and finish_export is None:
                    # Verification scratch has been released before serial export.
                    diagnostics.mark("cache_export")
                    with timings.stage("cache-export"):
                        accepted = cache.export_mounts()
                    record_cache_export(cache, accepted, env, cache_metrics)
        return True
    def superseded():
        require(env.get("CD_COMPONENT") in DEPLOYABLES, "Unexpected superseded component")
        step_output(env, "disposition", skip["disposition"])

    outcome = {}
    result = run_publication(operation, on_superseded=superseded, component=env.get("CD_COMPONENT"), outcome=outcome)
    if env.get("CD_WEB_CONTINUITY") == "required" and outcome.get("registry_failure"):
        registry_operation, status = outcome["registry_failure"]
        print(f"Web registry authorization refused operation={registry_operation} http_status={status}", flush=True)
    # run_private has restored the public descriptors; only fixed stage records
    # cross that boundary, including the failing phase on an unsuccessful run.
    component = env.get("CD_COMPONENT")
    if component in TARGETS:
        timings.emit(sys.stdout, scan_only=component in RUNTIME_TARGETS)
    # Every image reports its build summary, so a failed runtime build also
    # names the Dockerfile step where it stopped. BuildKit can mark a vertex
    # failed in a build that still completes (an optional cache import, for
    # example), so the error annotation is reserved for a publication that
    # actually stopped in its build stage.
    label = component if component in DEPLOYABLES else "image"
    stopped_in_build = bool(result) and outcome.get("stage") == "build"
    for metrics in build_metrics:
        # Failing test names are re-checked here too, like their annotations.
        value = public_summary(metrics) if metrics else {"complete": False, "reason": "progress-unavailable"}
        print(json.dumps({"event": "publication-build", **value}, sort_keys=True), flush=True)
        for step in value.get("failed_steps", ())[:1] if stopped_in_build else ():
            print(f"::error title=Image build step failed::{label} stopped at Dockerfile step {step}; "
                  "its output is withheld. Reproduce that step locally from the same source.", flush=True)
    if stopped_in_build and build_metrics:
        # The last build started is the one that stopped. A failed test stage
        # in it names its failing tests: names matched by fixed failure
        # patterns and re-validated here, never raw build output.
        annotate_failed_tests(label, build_metrics[-1], sys.stdout)
    for component in reused_native:
        print(f"Staging image reuse result=reserved-native-ci component={component} build=skipped", flush=True)
    if reused_prepared:
        print("Staging image reuse result=prepared-reservation", flush=True)
    for reason in reuse_misses:
        print("CI image reuse miss reason=" + reason, flush=True)
    if component in APP_TARGETS:
        emit_cache_metrics(component, cache_metrics, sys.stdout)
    if layer_exports and component in SCOPES:
        # Diagnostic only, after publication: did the last export reach the
        # Actions cache, or will the next build of this component start cold?
        started, progress = layer_exports[-1]
        try:
            report_layers(component, SCOPES[component], progress, started, env)
        except Exception:  # noqa: BLE001 - by construction, the check never changes the outcome.
            print(f"CACHE-SAVE cache=build-layers component={component} result=unknown reason=unreadable",
                  flush=True)
    return result


if __name__ == "__main__":
    raise SystemExit(main())
