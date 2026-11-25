#!/usr/bin/env python3
"""Load and verify the exact tested archive built by the qualification producer."""

import hashlib
import json
import os
from pathlib import Path
import subprocess
import sys

source = Path(sys.argv[1])
metadata = json.loads((source / "metadata.json").read_text())
archive_name = "cma-runtime-omarchy-linux-amd64.docker.tar.gz"
reference = "cma-runtime-omarchy:ci"
if (
    metadata["component"] != "runtime-omarchy"
    or metadata["sourceRevision"] != os.environ["GITHUB_SHA"]
    or metadata["workflow"]["runId"] != os.environ["GITHUB_RUN_ID"]
    or metadata["workflow"]["runAttempt"] != os.environ["GITHUB_RUN_ATTEMPT"]
    or metadata["testedReference"] != reference
    or metadata["archive"] != archive_name
):
    raise ValueError("The archive is from another qualification source or run attempt")
archive = source / archive_name
with archive.open("rb") as stream:
    digest = hashlib.file_digest(stream, "sha256").hexdigest()
if archive.stat().st_size != metadata["archiveBytes"] or digest != metadata["archiveSha256"]:
    raise ValueError("The downloaded archive differs from the producer")
subprocess.run(["docker", "load", "--input", str(archive)], check=True)
subprocess.run(["docker", "tag", metadata["imageId"], reference], check=True)
observed = subprocess.check_output(["docker", "image", "inspect", reference, "--format", "{{.Id}}"], text=True).strip()
if observed != metadata["imageId"]:
    raise ValueError("The loaded image differs from the producer")
