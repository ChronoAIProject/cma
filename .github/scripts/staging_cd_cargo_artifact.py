"""Authenticated develop-only transport for sealed AM and EG Cargo bundles.

Layer caches hold the repository's Actions cache at its storage limit, so the
optional Cargo save was never admitted and every Rust release compiled from
nothing. Develop Staging CD runs retain the sealed bundle as a run artifact
instead. A later run accepts one only from a verified producer job and with an
exact artifact digest; the existing bundle validator then decides between an
exact restore, an incremental-only graph fallback and a cold build.
"""
from datetime import datetime
import hashlib
from itertools import islice
import json
import os
from pathlib import Path
import re
import shutil
import stat
import subprocess
import time
import zipfile

import ci_retry
from staging_cd_build_cargo_keys import CARGO_NAMESPACE, CargoKey

REPO = "ChronoAIProject/cma"
WORKFLOW = ".github/workflows/staging-cd.yml"
COMPONENTS = {"am", "eg"}
FILES = ("manifest.json", "registry.tar.gz", "target.tar.gz")
MIB = 1024**2
# Run artifacts carry larger sealed archives than the 1 GiB Actions cache path.
# On 20 Nov 2026 AM's bundle was 1,015 MiB compressed and 2.56 GiB expanded in
# about 27k files; every later export was withheld. These bounds leave about 2x
# headroom, with the compressed bound binding first at that 2.6:1 ratio, while
# staying within a standard runner's disk. The 200k member bound is unchanged.
MAX_BUNDLE_BYTES = 2 * 1024**3
MAX_EXPANDED_BYTES = 6 * 1024**3
# The manifest is bounded to 4 KiB. Stored (uncompressed) ZIP framing for three
# members fits in the extra MiB.
MAX_ARTIFACT_BYTES = MAX_BUNDLE_BYTES + 4096 + MIB
CANDIDATES = 5
EXACT_CANDIDATES = 3
# An unavailable index must leave the ordinary family lookup its download budget.
EXACT_LOOKUP_SECONDS = 30
MAX_INDEX_BYTES = 64 * 1024
# Any workflow run may upload an artifact with this name. List enough rows to
# find the develop producers among them.
LISTING = 50
# A 2 GiB download needs about 6 MiB/s within DOWNLOAD_SECONDS; the overall
# deadline leaves room for the provenance reads before it.
DOWNLOAD_SECONDS = 360
DEADLINE_SECONDS = 420
CHUNK = 8 * MIB


class Refused(ValueError):
    """Diagnostics name a fixed reason, never remote or archive content."""


def require(value, reason="refused"):
    if not value:
        raise Refused(reason)


def artifact_name(component, family=None):
    require(component in COMPONENTS, "component")
    if family is None:
        return "cma-cd-cargo-" + component
    require(re.fullmatch(re.escape(CARGO_NAMESPACE + component + "-") + r"[0-9a-f]{64}-", family),
            "family")
    return family.removesuffix("-")


def index_name(component, family, graph):
    name = artifact_name(component, family)
    require(isinstance(graph, str) and re.fullmatch(r"[0-9a-f]{64}", graph), "graph")
    return f"graph-{name}-{graph}"


def context(env):
    require(env.get("GITHUB_REPOSITORY") == REPO and env.get("GITHUB_EVENT_NAME") == "push"
            and env.get("GITHUB_REF") == "refs/heads/develop" and env.get("CD_COMPONENT") in COMPONENTS,
            "context")
    require(re.fullmatch(r"[0-9a-f]{40}", env.get("GITHUB_SHA", "")), "context")
    for key in ("GITHUB_RUN_ID", "GITHUB_RUN_ATTEMPT"):
        require(re.fullmatch(r"[1-9][0-9]*", env.get(key, "")), "context")
    root = Path(env.get("RUNNER_TEMP", ""))
    require(root.is_absolute() and root.is_dir() and not root.is_symlink(), "context")
    return root / "cma-build-cache" / "accepted"


def github(path, *, timeout, output=None):
    require(path.startswith("repos/" + REPO + "/") and ".." not in path, "path")
    command = ["gh", "api", "--hostname", "github.com", "--method", "GET", path]
    if output is None:
        result = subprocess.run(command, capture_output=True, timeout=timeout, check=False)
        if result.returncode != 0:
            # The cause marks a transport failure as repeatable; the reason stays fixed.
            raise Refused("read") from ci_retry.cli_cause(result.stderr)
        require(len(result.stdout) <= 4 * MIB, "read")
        return json.loads(result.stdout)
    with output.open("xb") as stream:
        result = subprocess.run(command, stdout=stream, stderr=subprocess.PIPE, timeout=timeout, check=False)
    if result.returncode != 0:
        raise Refused("download") from ci_retry.cli_cause(result.stderr)
    return output


# Producers are recent develop push heads; older bundles have expired anyway.
FIRST_PARENT_WINDOW = 2000


def on_develop(root, source, head, timeout):
    # A tag push reports the tag's name as its branch, and merged PR commits are
    # also develop ancestors. Only a commit on develop's own first-parent line,
    # which every develop push head is, can be a producer.
    result = subprocess.run(["git", "rev-list", "--first-parent", f"--max-count={FIRST_PARENT_WINDOW}", head],
                            cwd=root, capture_output=True, text=True, timeout=timeout, check=False)
    return result.returncode == 0 and source in result.stdout.split()


def validate_artifact(artifact, name, *, max_bytes=MAX_ARTIFACT_BYTES):
    require(isinstance(artifact, dict) and artifact.get("name") == name
            and artifact.get("expired") is False and type(artifact.get("id")) is int and artifact["id"] > 0
            and type(artifact.get("size_in_bytes")) is int
            and 0 < artifact["size_in_bytes"] <= max_bytes
            and isinstance(artifact.get("digest"), str)
            and re.fullmatch(r"sha256:[a-f0-9]{64}", artifact["digest"]), "artifact")
    origin = artifact.get("workflow_run")
    require(isinstance(origin, dict) and type(origin.get("id")) is int and origin["id"] > 0
            and origin.get("head_branch") == "develop"
            and type(origin.get("repository_id")) is int
            and origin.get("head_repository_id") == origin["repository_id"]
            and isinstance(origin.get("head_sha"), str) and re.fullmatch(r"[0-9a-f]{40}", origin["head_sha"]),
            "artifact")
    return origin


def validate_producer(run, jobs, artifact, component):
    origin = artifact["workflow_run"]
    require(isinstance(run, dict) and run.get("repository", {}).get("full_name") == REPO
            and run.get("head_repository", {}).get("full_name") == REPO
            and run.get("path") == WORKFLOW and run.get("event") == "push"
            and run.get("head_branch") == "develop", "producer")
    repository = run["repository"].get("id")
    require(type(repository) is int and repository > 0 and run["head_repository"].get("id") == repository
            and origin.get("repository_id") == repository and origin.get("head_repository_id") == repository,
            "producer")
    source, run_id, attempt = run.get("head_sha"), run.get("id"), run.get("run_attempt")
    require(run_id == origin["id"] and source == origin["head_sha"]
            and type(attempt) is int and attempt > 0, "producer")
    require(isinstance(jobs, dict) and isinstance(jobs.get("jobs"), list)
            and jobs.get("total_count") == len(jobs["jobs"]), "producer")
    matches = [job for job in jobs["jobs"] if isinstance(job, dict)
               and job.get("name") == f"Release {component} / Publish"]
    # Only a completed, successful publication sealed and uploaded the bundle.
    require(len(matches) == 1 and matches[0].get("status") == "completed"
            and matches[0].get("conclusion") == "success" and matches[0].get("head_sha") == source
            and matches[0].get("run_id") == run_id and matches[0].get("run_attempt") == attempt, "producer")
    # A different family can leave an older attempt's artifact behind on a
    # rerun. Bind the upload to the successful Publish job of the same attempt.
    started, created, completed = (datetime.fromisoformat(value) for value in (
        matches[0].get("started_at"), artifact.get("created_at"), matches[0].get("completed_at")))
    require(all(instant.tzinfo is not None for instant in (started, created, completed))
            and started <= created <= completed, "producer")
    return {"producer_run": run_id, "producer_attempt": attempt, "producer_source": source}


def file_digest(path):
    value = hashlib.sha256()
    with path.open("rb") as stream:
        for block in iter(lambda: stream.read(CHUNK), b""):
            value.update(block)
    return "sha256:" + value.hexdigest()


def unpack(archive_path, artifact, destination, family, *, exact_graph=None):
    item = archive_path.lstat()
    require(stat.S_ISREG(item.st_mode) and item.st_size <= MAX_ARTIFACT_BYTES, "archive")
    require(file_digest(archive_path) == artifact["digest"], "digest")
    with zipfile.ZipFile(archive_path) as archive:
        members = archive.infolist()
        require(sorted(member.filename for member in members) == sorted(FILES), "members")
        total = 0
        for member in members:
            mode = (member.external_attr >> 16) & 0o170000
            require(not member.flag_bits & 1 and mode in (0, stat.S_IFREG)
                    and 0 < member.file_size <= MAX_ARTIFACT_BYTES, "members")
            total += member.file_size
        require(total <= MAX_ARTIFACT_BYTES, "members")
        # Check the sealed identity before extracting the archive mounts.
        manifest = archive.getinfo("manifest.json")
        require(manifest.file_size < 4096, "manifest")
        metadata = json.loads(archive.read(manifest))
        prefix = metadata.get("prefix") if isinstance(metadata, dict) else None
        require(isinstance(prefix, str) and re.fullmatch(re.escape(family) + r"[0-9a-f]{64}-", prefix),
                "family")
        if exact_graph is not None:
            format_id = metadata.get("format")
            require(type(format_id) is int and format_id in (1, 2), "graph")
            source = ""
            if format_id == 2:
                source = metadata.get("source")
                require(metadata.get("graph") == exact_graph and isinstance(source, str)
                        and re.fullmatch(r"[0-9a-f]{40}", source), "graph")
            require(prefix == CargoKey(family, exact_graph, source).prefix, "graph")
        destination.mkdir(mode=0o700)
        for member in members:
            with archive.open(member) as source, (destination / member.filename).open("xb") as target:
                shutil.copyfileobj(source, target, CHUNK)
    for name in FILES:
        written = (destination / name).lstat()
        require(stat.S_ISREG(written.st_mode) and written.st_nlink == 1, "members")


def discard(path):
    if path.is_symlink() or path.is_file():
        path.unlink()
    elif path.exists():
        shutil.rmtree(path)


FAILURES = (Refused, KeyError, TypeError, ValueError, AttributeError, EOFError, NotImplementedError, OSError,
            subprocess.SubprocessError, zipfile.BadZipFile, zipfile.LargeZipFile)


def restore(env, get=github, clock=time.monotonic, root=None, ancestry=on_develop, sleep=time.sleep):
    destination = context(env)
    component = env["CD_COMPONENT"]
    family = env.get("CD_CARGO_CACHE_FAMILY", "")
    names = (artifact_name(component, family), artifact_name(component))
    graph = env.get("CD_CARGO_CACHE_GRAPH")
    index = index_name(component, family, graph) if graph else None
    require(not destination.exists() and not destination.is_symlink(), "destination")
    destination.parent.mkdir(mode=0o700, exist_ok=True)
    require(destination.parent.is_dir() and not destination.parent.is_symlink(), "destination")
    started = clock()
    deadline = started + DEADLINE_SECONDS
    exact_deadline = started + EXACT_LOOKUP_SECONDS

    def remaining(cap, until=deadline):
        left = until - clock()
        require(left > 0, "deadline")
        return min(cap, left)

    def read(path, cap, output=None, *, until=deadline):
        # Every read is a GET; transport failures repeat inside the restore deadline.
        def attempt():
            if output is None:
                return get(path, timeout=remaining(cap, until))
            discard(output)  # a repeat never resumes a partial download
            return get(path, timeout=remaining(cap, until), output=output)
        return ci_retry.call(attempt, name="cargo-artifact-read", classify=ci_retry.classify,
                             deadline=until, clock=clock, sleep=sleep)

    def rows_for(name, until=deadline):
        listing = read(f"repos/{REPO}/actions/artifacts?name={name}&per_page={LISTING}", 10, until=until)
        require(isinstance(listing, dict) and isinstance(listing.get("artifacts"), list), "listing")
        rows = [row for row in listing["artifacts"] if isinstance(row, dict)
                and row.get("name") == name and isinstance(row.get("workflow_run"), dict)
                and row["workflow_run"].get("head_branch") == "develop"
                and type(row["workflow_run"].get("repository_id")) is int
                and row["workflow_run"].get("head_repository_id") == row["workflow_run"]["repository_id"]]
        return sorted(rows, key=lambda row: row.get("id") if type(row.get("id")) is int else 0, reverse=True)

    def candidates():
        # Family-specific names avoid fetching a multi-GiB archive from a
        # different toolchain. Legacy names bridge the artifact retention window.
        for name in names:
            if clock() >= deadline:
                return
            for row in rows_for(name):
                yield name, row

    root = Path.cwd() if root is None else root

    def producer_inputs(origin, until=deadline):
        require(str(origin["id"]) != env["GITHUB_RUN_ID"], "self")
        require(ancestry(root, origin["head_sha"], env["GITHUB_SHA"], remaining(10, until)), "ancestry")
        run = read(f"repos/{REPO}/actions/runs/{origin['id']}", 10, until=until)
        attempt = run.get("run_attempt") if isinstance(run, dict) else None
        require(type(attempt) is int and attempt > 0, "producer")
        jobs = read(f"repos/{REPO}/actions/runs/{origin['id']}/attempts/{attempt}/jobs?per_page=100", 10,
                    until=until)
        return run, jobs

    def exact_matches():
        # The small index's name is a discovery hint. The bulk artifact still
        # has to come from that same successful Publish attempt and seal this dependency graph.
        if index is None:
            return
        try:
            indexes = rows_for(index, exact_deadline)
        except FAILURES:
            return
        for marker in indexes[:EXACT_CANDIDATES]:
            if clock() >= exact_deadline:
                break
            try:
                origin = validate_artifact(marker, index, max_bytes=MAX_INDEX_BYTES)
                run, jobs = producer_inputs(origin, exact_deadline)
                producer = validate_producer(run, jobs, marker, component)
                path = f"repos/{REPO}/actions/runs/{origin['id']}/artifacts?name={names[0]}&per_page=2"
                listing = read(path, 10, until=exact_deadline)
                require(isinstance(listing, dict) and type(listing.get("total_count")) is int
                        and listing["total_count"] == 1
                        and isinstance(listing.get("artifacts"), list) and len(listing["artifacts"]) == 1,
                        "index")
                artifact = listing["artifacts"][0]
                require(validate_artifact(artifact, names[0]) == origin, "index")
                require(validate_producer(run, jobs, artifact, component) == producer, "index")
            except FAILURES:
                continue
            yield artifact, producer

    download = destination.parent / "artifact.zip"

    def restore_bundle(artifact, producer, *, exact_graph=None):
        try:
            read(f"repos/{REPO}/actions/artifacts/{artifact['id']}/zip", DOWNLOAD_SECONDS, download)
            unpack(download, artifact, destination, family, exact_graph=exact_graph)
        except FAILURES:
            discard(destination)
            raise
        finally:
            discard(download)
        return {"state": "RESTORED", **producer, "artifact_id": artifact["id"],
                "artifact_digest": artifact["digest"]}

    tried_exact = set()
    for artifact, producer in exact_matches():
        tried_exact.add(artifact["id"])
        try:
            return {**restore_bundle(artifact, producer, exact_graph=graph), "selection": "exact-graph"}
        except FAILURES:
            continue

    for name, artifact in islice(candidates(), CANDIDATES):
        try:
            origin = validate_artifact(artifact, name)
            if artifact["id"] in tried_exact:
                continue
            run, jobs = producer_inputs(origin)
            producer = validate_producer(run, jobs, artifact, component)
            return restore_bundle(artifact, producer)
        except FAILURES as error:
            if isinstance(error, Refused) and str(error) == "deadline":
                break
    return {"state": "MISS"}


def main():
    try:
        result = restore(os.environ)
    except FAILURES:
        result = {"state": "UNAVAILABLE"}
    print(json.dumps({"event": "cargo-artifact", "component": os.environ.get("CD_COMPONENT"), **result},
                     sort_keys=True), flush=True)


if __name__ == "__main__":
    main()
