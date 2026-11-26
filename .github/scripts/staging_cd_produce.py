"""Build the exact reserved release once, before the registry publisher runs."""

import json
import os
import sys
import tempfile
import time
from pathlib import Path

from staging_cd_admission import event_identity
from staging_cd_build import build, snapshot
from staging_cd_build_cache import SCOPES, BuildCache
from staging_cd_cache_save import report_layers
from staging_cd_common import require
from staging_cd_github import GitHub
from staging_cd_publication_diagnostics import run_publication
from staging_cd_publish import emit_cache_metrics, validated_tasks
from staging_cd_release_archive import ARCHIVE, MANIFEST, path_for, seal
from staging_cd_release_tags import local_tag_reader
from staging_cd_reservations import Reservations, ReservationSuperseded
from staging_cd_resolve import output as step_output
from staging_cd_test_failures import public_summary
from staging_cd_timing import Timings


def produce(env, root, diagnostics, progress, *, timings=None, cache_metrics=None, layer_exports=None):
    timings = timings or Timings()
    event_identity(env)
    require(env["GITHUB_EVENT_NAME"] == "push", "Image preparation only accepts develop pushes")
    tasks = validated_tasks(json.loads(env["CD_TASKS"]), env)
    pending = [value for mode, value in tasks if mode == "publish"]
    # Build one candidate at a time. The existing publisher interleaves builds
    # and uploads for a catch-up queue, bounding its storage by one archive.
    if len(pending) != 1:
        return "no-single-reservation"
    reservation = pending[0]
    if reservation["source"] != env["GITHUB_SHA"]:
        return "historical-reservation"

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
        cache = None
        if env.get("CD_BUILD_CACHE") == "enabled":
            diagnostics.mark("cache_restore")
            cache = BuildCache(root, env, source, reservation["component"], revision=reservation["source"])
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
        if cache is not None and cache.key is not None:
            seal(directory, release, cache.namespace, cache.cache_day)
        else:
            seal(directory, release)
        # The destination is allocated exclusively and the manifest is the
        # final commit marker. A partial handoff cannot authorize publication.
        destination.mkdir(mode=0o700, exist_ok=False)
        os.link(directory / ARCHIVE, destination / ARCHIVE)
        os.link(directory / MANIFEST, destination / MANIFEST)
    return "prepared"


def main():
    env = os.environ
    progress = {}
    disposition = []
    timings = Timings()
    cache_metrics = []
    layer_exports = []

    def run(_masker, diagnostics):
        disposition.append(produce(env, Path.cwd(), diagnostics, progress,
                                   timings=timings, cache_metrics=cache_metrics, layer_exports=layer_exports))
        return True

    result = run_publication(run, component=env.get("CD_COMPONENT"))
    if disposition:
        step_output(env, "prepared", "true" if disposition[0] == "prepared" else "false")
        print("Staging release producer result=" + disposition[0], flush=True)
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
    sys.exit(main())
