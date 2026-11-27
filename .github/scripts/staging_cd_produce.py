"""Prepare the exact reserved release before the registry publisher runs."""

import json
import os
import sys
import tempfile
import time
from pathlib import Path

from staging_cd_admission import event_identity
from staging_cd_artifacts import try_reuse
from staging_cd_build import build, command, identity, snapshot
from staging_cd_build_cache import SCOPES, BuildCache
from staging_cd_cache_save import report_layers
from staging_cd_native_archive import COMPONENTS, METADATA, artifact_name, stamp_metadata
from staging_cd_common import require
from staging_cd_github import GitHub
from staging_cd_publication_diagnostics import run_publication
from staging_cd_publish import emit_cache_metrics, record_cache_export, validated_tasks
from staging_cd_release_archive import ARCHIVE, MANIFEST, path_for, seal
from staging_cd_release_tags import local_tag_reader
from staging_cd_reservations import Reservations, ReservationSuperseded
from staging_cd_resolve import output as step_output
from staging_cd_test_failures import public_summary
from staging_cd_timing import Timings


def planned(env):
    event_identity(env)
    require(env["GITHUB_EVENT_NAME"] == "push", "Image preparation only accepts develop pushes")
    return validated_tasks(json.loads(env["CD_TASKS"]), env)


def select_native(env):
    def run(_masker, _diagnostics):
        tasks = planned(env)
        require(env["CD_COMPONENT"] in COMPONENTS, "Reserved native producer component differs")
        select = len(tasks) == 1 and tasks[0][0] == "publish" and tasks[0][1]["source"] == env["GITHUB_SHA"]
        step_output(env, "build", "true" if select else "false")
        return True

    return run_publication(run, component=env.get("CD_COMPONENT"))


def produce(env, root, diagnostics, progress, *, timings=None, cache_metrics=None, layer_exports=None,
            on_reuse_miss=None):
    timings = timings or Timings()
    native = env.get("CD_NATIVE_CI_PRODUCER") == "true"
    require(not native or env["CD_COMPONENT"] in COMPONENTS, "Reserved native producer component differs")
    tasks = planned(env)
    pending = [value for mode, value in tasks if mode == "publish"]
    # Build one candidate at a time. The existing publisher interleaves builds
    # and uploads for a catch-up queue, bounding its storage by one archive.
    if len(pending) != 1 or (native and len(tasks) != 1):
        return "no-single-reservation"
    reservation = pending[0]
    historical_web = reservation["component"] == "web" and env.get("CD_WEB_CONTINUITY_RAW") == "true"
    if reservation["source"] != env["GITHUB_SHA"] and not historical_web:
        return "historical-reservation"

    if native:
        head, tree = command(["git", "rev-parse", "HEAD", "HEAD^{tree}"], cwd=root, timeout=10).decode().splitlines()
        require((head, tree) == (reservation["source"], reservation["tree"]),
                "Native producer checkout differs from the reservation")

    token = env.get("GH_TOKEN")
    api = GitHub(token, local_tags=local_tag_reader(root))
    authority = Reservations(api, token, root)
    try:
        authority.require_current(reservation, build_started=False)
    except ReservationSuperseded:
        return "reservation-superseded"

    parent = Path(env["RUNNER_TEMP"]) / "cma-prepared-release"
    parent.mkdir(mode=0o700, exist_ok=True)
    require(not parent.is_symlink(), "Prepared release parent is invalid")
    destination = path_for(parent, reservation)
    require(not destination.exists() and not destination.is_symlink(), "Prepared release destination already exists")
    with tempfile.TemporaryDirectory(prefix="release-", dir=parent) as temporary:
        directory = Path(temporary) / "candidate"
        directory.mkdir()
        source = Path(temporary) / "source"
        diagnostics.mark("snapshot")
        with timings.stage("source"):
            snapshot(root, reservation["source"], source)
        archive = None
        if not native and reservation["component"] in COMPONENTS:
            release, _, _ = identity(reservation, source)
            diagnostics.mark("reuse")
            with timings.stage("reuse"):
                # The publisher requires the exact reserved label on a sealed handoff.
                archive = try_reuse(release, directory, api, token, source_root=source,
                                    on_miss=on_reuse_miss, require_reserved_label=True)
        cache = None
        if archive is None:
            if env.get("CD_BUILD_CACHE") == "enabled":
                diagnostics.mark("cache_restore")
                cache = BuildCache(root, env, source, reservation["component"])
                with timings.stage("cache-restore"):
                    cache.restore()
                if cache.key is not None and cache_metrics is not None:
                    cache_metrics.append({"operation": "restore", "result": cache.restore_result})
            diagnostics.mark("build")
            started = time.time()
            with timings.stage("build"):
                _, release = build(root, reservation, directory, source=source, cache=cache, progress=progress)
            if layer_exports is not None and cache is not None and cache.exports_layers():
                layer_exports.append(started)
        if not native and cache is not None and cache.key is not None:
            seal(directory, release, cache.namespace, cache.cache_day)
        else:
            seal(directory, release)
        if native:
            diagnostics.mark("inspect")
            tested = stamp_metadata(directory, release, source)
            authority.require_current(reservation, build_started=True)
            step_output(env, "native-name", artifact_name(release))
        # The destination is allocated exclusively and the manifest is the
        # final commit marker. A partial handoff cannot authorize publication.
        destination.mkdir(mode=0o700, exist_ok=False)
        os.link(directory / ARCHIVE, destination / ARCHIVE)
        os.link(directory / MANIFEST, destination / MANIFEST)
        if native:
            os.link(directory / METADATA, destination / METADATA)
            step_output(env, "native-path", str(destination))
            qualification = {"component": release["component"], "source": release["source"],
                            "version": tested["versionLabel"], "imageId": tested["imageId"],
                            "buildFingerprint": tested["buildFingerprint"]}
    if native and cache is not None and cache.key is not None and cache_metrics is not None:
        diagnostics.mark("cache_export")
        with timings.stage("cache-export"):
            accepted = cache.export_mounts()
        record_cache_export(cache, accepted, env, cache_metrics)
    if native:
        return qualification
    return "prepared" if archive is None else "prepared-ci-image"


def main():
    env = os.environ
    progress = {}
    disposition = []
    timings = Timings()
    cache_metrics = []
    layer_exports = []
    reuse_misses = []

    def run(_masker, diagnostics):
        disposition.append(produce(env, Path.cwd(), diagnostics, progress,
                                   timings=timings, cache_metrics=cache_metrics, layer_exports=layer_exports,
                                   on_reuse_miss=reuse_misses.append))
        return True

    result = run_publication(run, component=env.get("CD_COMPONENT"))
    if disposition:
        value = disposition[0]
        if isinstance(value, dict):
            print("Staging native image tested " + json.dumps(value, sort_keys=True), flush=True)
            value = "prepared-native"
        step_output(env, "prepared", "true" if value in {"prepared", "prepared-ci-image"} else "false")
        print("Staging release producer result=" + value, flush=True)
    for reason in reuse_misses:
        print("CI image reuse miss reason=" + reason, flush=True)
    timings.emit(sys.stdout)
    component = env.get("CD_COMPONENT")
    emit_cache_metrics(component, cache_metrics, sys.stdout)
    if layer_exports:
        report_layers(component, SCOPES[component], progress, layer_exports[0], env)
    if progress:
        print(json.dumps({"event": "publication-build", **public_summary(progress)}, sort_keys=True),
              flush=True)
    return result


if __name__ == "__main__":
    sys.exit(select_native(os.environ) if sys.argv[1:] == ["--select-native"] else main())
