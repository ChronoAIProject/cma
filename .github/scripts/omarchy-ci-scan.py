"""Run the existing Omarchy image scanner while foreground CI gates run."""

import argparse
import hashlib
import json
import os
from pathlib import Path
import subprocess
import sys
import time


ROOT = Path(__file__).resolve().parent
SCAN_TIMEOUT = 30 * 60
COMPONENT = "runtime-omarchy"
REFERENCE = "cma-runtime-omarchy:ci"
ARCHIVE = "cma-runtime-omarchy-linux-amd64.docker.tar.gz"


def require(condition, message):
    if not condition:
        raise ValueError(message)


def identity(source, revision):
    metadata = source / "metadata.json"
    document = json.loads(metadata.read_bytes())
    workflow = document["workflow"]
    require(document["component"] == COMPONENT and document["sourceRevision"] == revision
            and document["archive"] == ARCHIVE and document["testedReference"] == REFERENCE
            and workflow["runId"] == os.environ["GITHUB_RUN_ID"]
            and workflow["runAttempt"] == os.environ["GITHUB_RUN_ATTEMPT"],
            "the private scan input belongs to another image or run")
    return document


def start(source, state, revision):
    identity(source, revision)
    state.mkdir(mode=0o700)
    (state / "deadline").write_text(str(time.monotonic() + SCAN_TIMEOUT + 5))
    env = dict(os.environ, GITHUB_STEP_SUMMARY=str(state / "summary.md"))
    with (state / "scanner.log").open("xb") as output:
        subprocess.Popen([sys.executable, "-B", str(Path(__file__).resolve()), "worker", revision],
                         stdin=subprocess.DEVNULL, stdout=output, stderr=subprocess.STDOUT,
                         env=env, start_new_session=True)


def scan(source, state, revision):
    metadata = identity(source, revision)
    result = subprocess.run(
        ["timeout", "--kill-after=5", str(SCAN_TIMEOUT), sys.executable, "-B",
         str(ROOT / "ci_image_secret_scan.py"), COMPONENT, str(source), revision],
        check=False,
    )
    pending = state / "result.pending"
    pending.write_text(json.dumps({"image": metadata, "exit_code": result.returncode}))
    pending.rename(state / "result.json")


def promote(source, state, destination, revision):
    end = min(float((state / "deadline").read_text()), time.monotonic() + SCAN_TIMEOUT + 5)
    report = state / "result.json"
    while not report.exists():
        require(time.monotonic() < end, "the image scanner did not record a result before its deadline")
        time.sleep(1)
    print((state / "scanner.log").read_text(), end="", flush=True)
    summary = state / "summary.md"
    if summary.exists() and os.environ.get("GITHUB_STEP_SUMMARY"):
        with open(os.environ["GITHUB_STEP_SUMMARY"], "a", encoding="utf-8") as output:
            output.write(summary.read_text())
    result = json.loads(report.read_bytes())
    require(result["exit_code"] == 0, "the Omarchy image secret scan failed")
    metadata = identity(source, revision)
    require(result["image"] == metadata, "the scan result describes a different image")
    image_id = subprocess.check_output(
        ["docker", "image", "inspect", REFERENCE, "--format", "{{.Id}}"],
        text=True, timeout=30,
    ).strip()
    require(image_id == metadata["imageId"], "the tested image changed after the scan started")
    archive = source / ARCHIVE
    with archive.open("rb") as stream:
        digest = hashlib.file_digest(stream, "sha256").hexdigest()
    require(archive.stat().st_size == metadata["archiveBytes"]
            and digest == metadata["archiveSha256"], "the scan input archive changed")
    require(not destination.exists(), "the tested native artifact directory already exists")
    source.rename(destination)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("action", choices=("start", "worker", "promote"))
    parser.add_argument("revision")
    args = parser.parse_args()
    root = Path(os.environ["RUNNER_TEMP"])
    source = root / "omarchy-ci-scan-input"
    state = root / "omarchy-ci-scan-state"
    if args.action == "start":
        start(source, state, args.revision)
    elif args.action == "worker":
        scan(source, state, args.revision)
    else:
        promote(source, state, root / "ci-native-image", args.revision)


if __name__ == "__main__":
    main()
