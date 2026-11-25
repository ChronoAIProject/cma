"""Restore one artifact of this workflow run within fixed time bounds.

actions/download-artifact cannot be bounded inside a composite action: the
runner's action schema gives composite steps no timeout-minutes, and the action
itself aborts only a stream that sends nothing for 30 seconds. A 1.1 GB Rust
test archive once trickled for 76 minutes and spent its job's whole timeout.

Here each request has a wall-clock bound, and a timed-out, broken or transient
attempt starts again on a fresh connection a fixed number of times. Nothing is
extracted until the bytes match the size and SHA-256 digest GitHub recorded at
upload, the check `digest-mismatch: error` made. A final failure names the
artifact, how many attempts were made and the bound they hit.
"""
import argparse
import hashlib
import io
import json
import os
import re
import shutil
import stat
import subprocess
import sys
import tempfile
import threading
import time
import zipfile
from pathlib import Path, PurePosixPath

import ci_retry
from ci_outcome import error_site, report
from staging_cd_common import Refusal, matches, require

ATTEMPTS = 3
# 180 restores of the 1.1 GB Rust test archive took a median 44 s and at most
# 190 s, extraction included. Six minutes keeps a floor near 3 MB/s; a slower
# stream is a stall that a fresh connection usually clears.
ATTEMPT_SECONDS = 360
LIST_SECONDS = 30
MAX_ARTIFACT = 4 * 1024 ** 3
MAX_EXPANDED = 8 * 1024 ** 3
MAX_LISTING = 8 * 1024 ** 2
MAX_HEADERS = 64 * 1024
CHUNK = 1024 * 1024
SUBJECT = "artifact-download"
ARTIFACT_NAME = r"[A-Za-z0-9][A-Za-z0-9._-]{0,127}"
REPOSITORY = r"[A-Za-z0-9_.-]{1,100}/[A-Za-z0-9_.-]{1,100}"
STATUS_LINE = re.compile(r"HTTP/[0-9.]+ ([1-5][0-9]{2})\b")


class HTTPRefusal(Refusal):
    """GitHub answered with an HTTP error; its status and headers, never its text, classify it."""

    status = None


class StreamRefusal(Refusal):
    """gh ended before a complete response arrived."""


class AbsentRefusal(Refusal):
    """The run lists no live artifact with this name."""


class ExpiredRefusal(Refusal):
    """The artifact outlived its retention; only a new upload replaces it."""


def gh(path, env, sink, timeout, *, limit, streamed):
    """One read-only GitHub API request through gh, killed after `timeout` seconds.

    `--include` puts the final response's status line and headers before its
    body, so an HTTP error is classified as the shared retry policy classifies
    any response: by status, with throttling read from Retry-After and the
    rate-limit headers. Only the body reaches `sink`. When `streamed`, the
    request's only I/O is the remote stream, so gh failing without an HTTP
    error, such as a dropped connection, is transient.
    """
    command = ["gh", "api", "--include", path]
    with tempfile.TemporaryFile() as errors:
        process = subprocess.Popen(command, env=env, stdout=subprocess.PIPE, stderr=errors)
        expired = threading.Event()

        def stop():
            expired.set()
            process.kill()

        timer = threading.Timer(timeout, stop)
        timer.start()
        head, buffered, received = None, b"", 0
        try:
            for chunk in iter(lambda: process.stdout.read1(CHUNK), b""):
                if head is None:
                    buffered += chunk
                    end = buffered.find(b"\r\n\r\n")
                    require(end >= 0 or len(buffered) <= MAX_HEADERS, "GitHub response headers exceed their bound")
                    if end < 0:
                        continue
                    head, chunk = buffered[:end].decode("latin-1"), buffered[end + 4:]
                received += len(chunk)
                require(received <= limit, "GitHub response exceeds its bound")
                sink.write(chunk)
            process.wait()
        finally:
            timer.cancel()
            if process.poll() is None:
                process.kill()
                process.wait()
            process.stdout.close()
        if expired.is_set():
            raise subprocess.TimeoutExpired(command, timeout)
        status = STATUS_LINE.match(head or "")
        code = int(status[1]) if status else None
        if code is not None and code >= 400:
            error = HTTPRefusal("GitHub answered with an HTTP error")
            error.status = code
            raise error from ci_retry.status_cause(code, head)
        if process.returncode != 0:
            errors.seek(0)
            text = errors.read()[-ci_retry.SCAN_LIMIT:]
            # A response that began and then broke failed in transport, wherever it was going.
            cause = ci_retry.TransientError() if streamed or code is not None else ci_retry.cli_cause(text)
            raise StreamRefusal("GitHub response ended before it was complete") from cause
        require(code is not None and 200 <= code < 300, "GitHub response has no success status")


def locate(name, repository, run_id, env, timeout, policy):
    """The newest live artifact of this run with exactly this name."""
    path = f"repos/{repository}/actions/runs/{run_id}/artifacts?name={name}&per_page=100"

    def read():
        body = io.BytesIO()
        gh(path, env, body, timeout, limit=MAX_LISTING, streamed=False)
        listing = json.loads(body.getvalue())
        require(isinstance(listing, dict) and isinstance(listing.get("artifacts"), list)
                and listing.get("total_count") == len(listing["artifacts"]), "Artifact listing is incomplete")
        named = [item for item in listing["artifacts"] if isinstance(item, dict) and item.get("name") == name
                 and type(item.get("id")) is int and isinstance(item.get("workflow_run"), dict)
                 and item["workflow_run"].get("id") == run_id]
        found = [item for item in named if item.get("expired") is False]
        if not found and any(item.get("expired") is True for item in named):
            raise ExpiredRefusal("Artifact expired with its retention period")
        if not found:
            # The producing job finished before this one started, so a missing
            # entry can only be a listing that has not caught up yet.
            raise AbsentRefusal("Artifact is not listed in this workflow run") from ci_retry.TransientError()
        return max(found, key=lambda item: item["id"])

    artifact = ci_retry.call(read, name="artifact-list", classify=ci_retry.classify, honour_retry_after=True, **policy)
    require(artifact["id"] > 0 and type(artifact.get("size_in_bytes")) is int
            and 0 < artifact["size_in_bytes"] <= MAX_ARTIFACT, "Artifact size is outside its bound")
    require(matches(r"sha256:[0-9a-f]{64}", artifact.get("digest")), "Artifact has no recorded SHA-256 digest")
    return artifact


def download(artifact, archive, repository, env, results, *, name, attempts, timeout, policy, out):
    """Fetch the archive into a new file; each attempt removes its own partial bytes."""
    path = f"repos/{repository}/actions/artifacts/{artifact['id']}/zip"

    def attempt():
        stream = archive.open("xb")  # never adopts bytes this attempt did not write
        try:
            with stream:
                gh(path, env, stream, timeout, limit=artifact["size_in_bytes"], streamed=True)
        except BaseException as error:
            archive.unlink(missing_ok=True)
            results.append("timeout" if isinstance(error, subprocess.TimeoutExpired) else "failed")
            if results[-1] == "timeout":
                print(f"::warning title=Artifact download attempt timed out::{name}: attempt {len(results)} "
                      f"of {attempts} exceeded its {timeout}-second bound and was stopped", file=out, flush=True)
            raise
        results.append("success")

    ci_retry.call(attempt, name="artifact-download", classify=ci_retry.classify, attempts=attempts,
                  honour_retry_after=True, **policy)


def verify(archive, artifact):
    size, digest = 0, hashlib.sha256()
    with archive.open("rb") as stream:
        for chunk in iter(lambda: stream.read(CHUNK), b""):
            size += len(chunk)
            digest.update(chunk)
    require(size == artifact["size_in_bytes"], "Downloaded artifact size differs from its record")
    require("sha256:" + digest.hexdigest() == artifact["digest"], "Downloaded artifact digest differs from its record")


def extract(archive, destination):
    """Unpack regular files under destination, as the action lays them out."""
    with zipfile.ZipFile(archive) as source:
        entries, names, total = source.infolist(), set(), 0
        for entry in entries:
            parts = PurePosixPath(entry.filename).parts
            require(entry.filename and not entry.filename.startswith("/") and "\\" not in entry.filename
                    and ".." not in parts and entry.filename not in names
                    and not stat.S_ISLNK(entry.external_attr >> 16), "Artifact holds an unsafe entry")
            names.add(entry.filename)
            total += entry.file_size
        require(total <= MAX_EXPANDED, "Artifact expands beyond its bound")
        for entry in entries:
            target = destination.joinpath(*PurePosixPath(entry.filename).parts)
            if entry.is_dir():
                target.mkdir(parents=True, exist_ok=True)
                continue
            target.parent.mkdir(parents=True, exist_ok=True)
            if target.is_symlink() or target.exists():
                target.unlink()  # replaced, never written through
            with source.open(entry) as incoming, target.open("xb") as outgoing:
                shutil.copyfileobj(incoming, outgoing, CHUNK)


def outcome(error):
    """The "step" catalogue code that explains why the restore ended."""
    if isinstance(error, subprocess.TimeoutExpired):
        return "timeout"
    if isinstance(error, HTTPRefusal):
        if ci_retry.classify(error) == ci_retry.TRANSIENT:
            return "http-server"
        return "http-denied" if error.status in (401, 403) else "http-client"
    if isinstance(error, StreamRefusal):
        return "network"
    if isinstance(error, Refusal):
        return "refused"
    if isinstance(error, OSError):
        return "os-error"
    return "error"


def explanation(name, phase, results, attempts, timeout, error):
    if phase == "request":
        return f"{name} cannot be requested: the artifact name, repository or run id is invalid."
    if isinstance(error, ExpiredRefusal):
        return f"{name} has expired with its retention period. Re-run all jobs so it is built and uploaded again."
    if outcome(error) == "http-denied":
        return (f"GitHub refused this job's token for {name} (HTTP {error.status}). The job must grant "
                "actions: read, and so must every job that calls its workflow.")
    if isinstance(error, AbsentRefusal):
        return f"{name} is not listed in this workflow run; its producing job did not upload it."
    timed_out = results.count("timeout")
    if phase == "download" and timed_out == len(results) == attempts:
        return f"{name} was not downloaded: it timed out {attempts} times, each attempt stopped after {timeout} seconds."
    if phase == "download":
        return (f"{name} was not downloaded after {len(results)} of at most {attempts} attempts, {timed_out} of "
                f"them stopped after {timeout} seconds. Errors that are not transient are not repeated.")
    if phase == "verify":
        return (f"{name} was downloaded, but its size or SHA-256 digest differs from the record GitHub kept "
                "at upload. Nothing was extracted.")
    if phase == "extract":
        return f"{name} matched its recorded digest but could not be extracted."
    return f"The artifacts of this workflow run could not be listed, or {name} has no recorded SHA-256 digest."


def restore(name, destination, *, repository, run_id, env, out, attempts=ATTEMPTS,
            attempt_seconds=ATTEMPT_SECONDS, list_seconds=LIST_SECONDS, retry=None):
    """Download, verify and extract one artifact; return the process status."""
    policy, results, phase, started = dict(retry or {}), [], "request", time.monotonic()
    try:
        require(matches(ARTIFACT_NAME, name) and matches(REPOSITORY, repository) and type(run_id) is int
                and run_id > 0, "Artifact request is invalid")
        phase = "list"
        artifact = locate(name, repository, run_id, env, list_seconds, policy)
        destination.mkdir(parents=True, exist_ok=True)
        with tempfile.TemporaryDirectory(prefix=".artifact-download-", dir=destination) as work:
            archive = Path(work) / "artifact.zip"
            phase = "download"
            download(artifact, archive, repository, env, results, name=name, attempts=attempts,
                     timeout=attempt_seconds, policy=policy, out=out)
            phase = "verify"
            verify(archive, artifact)
            phase = "extract"
            extract(archive, destination)
    except Exception as error:  # noqa: BLE001 - explained by fixed fields, then the step fails.
        shown = name if matches(ARTIFACT_NAME, name) else "The requested artifact"
        message = explanation(shown, phase, results, attempts, attempt_seconds, error)
        print(f"::error title=Artifact download failed::{message}", file=out, flush=True)
        report("step", outcome(error), subject=SUBJECT, site=error_site(error),
               attempts=len(results) or None, stream=out)
        return 1
    print(f"Restored {name}: {artifact['size_in_bytes']} bytes matching {artifact['digest']} after "
          f"{len(results)} download attempt(s) in {time.monotonic() - started:.0f} s", file=out, flush=True)
    return 0


def main(argv=None):
    parser = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    parser.add_argument("--name", required=True, help="artifact name in this workflow run")
    parser.add_argument("--path", required=True, type=Path, help="directory the artifact is extracted into")
    args = parser.parse_args(argv)
    env = {key: value for key, value in os.environ.items() if key in ("PATH", "HOME", "LANG", "LC_ALL")}
    env.update(GH_TOKEN=os.environ.get("GH_TOKEN", ""), GH_HOST="github.com", GH_PROMPT_DISABLED="1",
               GH_NO_UPDATE_NOTIFIER="1")
    run_id = os.environ.get("GITHUB_RUN_ID", "")
    return restore(args.name, args.path, repository=os.environ.get("GITHUB_REPOSITORY", ""),
                   run_id=int(run_id) if run_id.isdigit() else 0, env=env, out=sys.stdout)


if __name__ == "__main__":
    raise SystemExit(main())
