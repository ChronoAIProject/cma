#!/usr/bin/env python3
"""Qualify the scanner extracted from one exact, unstarted API image."""
import hashlib
import json
import os
from pathlib import Path
import re
import signal
import subprocess
import sys
import tempfile
import time
import uuid

ROOT = Path(__file__).resolve().parents[2]
FILES = ("source.txt", "inputs.sha256", "binary.sha256", "buildinfo.txt")
CASES = ["clean values", "generic detection", "package token detection", "annotation cannot bypass detection"]
SOURCE = """CMA security rebuild of upstream Gitleaks 8.30.1
source=https://github.com/gitleaks/gitleaks
revision=83d9cd684c87d95d656c1458ef04895a7f1cbd8e
archive.sha256=6b2638a733b85619dc80bdf28e84e4fed7e526a761ab5c148fbf67695aea2115
builder=golang:1.26.11-alpine3.23@sha256:23f198931d4bd83b4f5a613bd75656257ae8bf832dcbc2db80e1050b5219ca2a
"""
INPUTS = """0d21d8af0e74395d5b88a1fd5ca4f421818c45b3f7a2b38100566eeca61ad675  go.mod
c1f21f073cc203c7678f230b9cb14131a25729f3e2fdbd73475d0de44be5711e  go.sum
e163e53b9e7e8a8511e77271e2b323ed057759542a6d988258afe3a1fa329caf  config/gitleaks.toml
"""


def require(value, message):
    if not value:
        raise ValueError(message)


def digest(path):
    result = hashlib.sha256()
    with path.open("rb") as source:
        for block in iter(lambda: source.read(1024 * 1024), b""):
            result.update(block)
    return result.hexdigest()


def command(arguments, **kwargs):
    return subprocess.run(arguments, check=True, stdout=subprocess.PIPE, stderr=subprocess.PIPE,
                          text=True, timeout=60, **kwargs).stdout


def inspect_image(image, source):
    require(re.fullmatch(r"sha256:[0-9a-f]{64}", image), "an exact built image ID is required")
    rows = json.loads(command(["docker", "image", "inspect", image]))
    require(len(rows) == 1, "image identity is ambiguous")
    row = rows[0]
    require(row["Id"] == image and row["Os"] == "linux" and row["Architecture"] == "amd64",
            "image identity or platform differs")
    labels = row["Config"]["Labels"]
    require(labels.get("org.opencontainers.image.title") == "CMA Agent Manager" and
            labels.get("org.opencontainers.image.source") == "https://github.com/ChronoAIProject/cma" and
            labels.get("org.opencontainers.image.revision") == source and
            labels.get("org.opencontainers.image.version") == "git-" + source[:12], "image source or component differs")
    return {"id": image, "platform": "linux/amd64", "source": source}


def extract(image, directory, receipt):
    name = "cma-scanner-qualification-" + uuid.uuid4().hex
    label = "dev.chrono.cma.scanner-qualification=" + name
    receipt["container_cleanup"] = False
    try:
        identifier = command(["docker", "create", "--name", name, "--label", label, "--network", "none",
                              "--entrypoint", "/bin/true", image]).strip()
        require(re.fullmatch(r"[0-9a-f]{64}", identifier), "invalid extraction container identity")
        rows = json.loads(command(["docker", "inspect", identifier]))
        require(len(rows) == 1 and rows[0]["Image"] == image and rows[0]["Name"] == "/" + name and
                rows[0]["Config"]["Labels"].get(label.split("=")[0]) == name and
                rows[0]["State"]["Status"] == "created" and rows[0]["State"]["Running"] is False,
                "extraction container identity or state differs")
        command(["docker", "cp", identifier + ":/usr/local/bin/gitleaks", str(directory / "gitleaks")])
        for filename in FILES:
            command(["docker", "cp", identifier + ":/usr/local/share/gitleaks/" + filename, str(directory / filename)])
    finally:
        identifiers = command(["docker", "ps", "--all", "--quiet", "--no-trunc", "--filter", "label=" + label,
                               "--filter", "name=^/" + name + "$"]).split()
        require(len(identifiers) <= 1 and all(re.fullmatch(r"[0-9a-f]{64}", item) for item in identifiers),
                "extraction cleanup identity is ambiguous")
        for identifier in identifiers:
            command(["docker", "rm", "--force", identifier])
        require(not command(["docker", "ps", "--all", "--quiet", "--filter", "label=" + label]).strip(),
                "extraction container remains")
        receipt["container_cleanup"] = True


def provenance(directory):
    for filename in ("gitleaks", *FILES):
        path = directory / filename
        require(path.is_file() and not path.is_symlink() and 0 < path.stat().st_size <=
                (128 * 1024 * 1024 if filename == "gitleaks" else 64 * 1024), "missing or invalid scanner artifact")
    binary = directory / "gitleaks"
    with binary.open("rb") as source:
        header = source.read(20)
    require(header[:6] == b"\x7fELF\x02\x01" and header[18:20] == b"\x3e\x00", "scanner is not Linux amd64 ELF")
    require(os.access(binary, os.X_OK), "scanner artifact is not executable")
    sha = digest(binary)
    require((directory / "binary.sha256").read_text() == sha + "  /usr/local/bin/gitleaks\n", "scanner binary hash differs")
    require((directory / "source.txt").read_text() == SOURCE and (directory / "inputs.sha256").read_text() == INPUTS,
            "scanner source or input pins differ")
    lines = (directory / "buildinfo.txt").read_text().splitlines()
    require(lines[0] == "/usr/local/bin/gitleaks: go1.26.11", "scanner toolchain differs")
    builds, deps = {}, {}
    for line in lines[1:]:
        fields = line.split()
        if fields and fields[0] == "build":
            key, value = fields[1].split("=", 1)
            require(key not in builds, "duplicate build setting")
            builds[key] = value
        if fields and fields[0] == "dep":
            require(fields[1] not in deps, "duplicate module")
            deps[fields[1]] = fields[2]
    require(all(builds.get(key) == value for key, value in {
        "-tags": "gore2regex", "CGO_ENABLED": "0", "GOOS": "linux", "GOARCH": "amd64"}.items()), "scanner build platform or flags differ")
    require(all(deps.get(key) == value for key, value in {"golang.org/x/crypto": "v0.55.0",
        "golang.org/x/text": "v0.41.0", "github.com/klauspost/compress": "v1.18.7"}.items()), "scanner patched modules differ")
    return {"binary_sha256": sha, "version": "8.30.1", "provenance_sha256": {name: digest(directory / name) for name in FILES}}


def qualify(image, source, output):
    receipt = {"schema": "cma-api-scanner-qualification-v1", "passed": False, "attempt": 1,
               "started_ms": time.time_ns() // 1_000_000, "kind": "rebuilt-api-image-cli"}
    directory = None
    try:
        require(re.fullmatch(r"[0-9a-f]{40}", source), "invalid source identity")
        require(command(["git", "rev-parse", "HEAD"], cwd=ROOT).strip() == source and
                not command(["git", "status", "--porcelain", "--untracked-files=no"], cwd=ROOT), "checkout source differs")
        receipt["image"] = inspect_image(image, source)
        receipt["tree"] = command(["git", "rev-parse", "HEAD^{tree}"], cwd=ROOT).strip()
        harness = ROOT / ".github/secret-scanner-build-tests.sh"
        receipt["harness_sha256"] = digest(harness)
        with tempfile.TemporaryDirectory(prefix="cma-api-scanner-") as temporary:
            directory = Path(temporary)
            extract(image, directory, receipt)
            receipt["scanner"] = provenance(directory)
            text = command(["bash", str(harness), "--require-real-binary"], cwd=ROOT,
                           env={**os.environ, "CMA_TEST_GITLEAKS": str(directory / "gitleaks")})
            cases = [line.removeprefix("SCANNER_CASE_PASS=") for line in text.splitlines() if line.startswith("SCANNER_CASE_PASS=")]
            require(cases == CASES and not any(line.startswith("skip") for line in text.splitlines()), "scanner cases missing, duplicated or skipped")
            require(digest(directory / "gitleaks") == receipt["scanner"]["binary_sha256"], "scanner artifact changed during execution")
            receipt["cases"] = cases
        receipt["temporary_cleanup"] = not directory.exists()
        require(receipt["temporary_cleanup"] and receipt["container_cleanup"], "owned cleanup incomplete")
        receipt["passed"] = True
    except (OSError, ValueError, KeyError, IndexError, TypeError, subprocess.SubprocessError, KeyboardInterrupt) as error:
        receipt["failure"] = str(error) if isinstance(error, ValueError) else type(error).__name__
        if isinstance(error, subprocess.CalledProcessError):
            receipt["failed_command_exit"] = error.returncode
    finally:
        if directory is not None:
            receipt["temporary_cleanup"] = not directory.exists()
    receipt["finished_ms"] = time.time_ns() // 1_000_000
    output.write_text(json.dumps(receipt, indent=2) + "\n")
    print(json.dumps(receipt))
    return 0 if receipt["passed"] else 1


if __name__ == "__main__":
    def interrupted(_number, _frame):
        raise KeyboardInterrupt
    signal.signal(signal.SIGTERM, interrupted)
    os.umask(0o077)
    sys.exit(qualify(os.environ["CMA_SCANNER_IMAGE_ID"], os.environ["GITHUB_SHA"],
                    Path(os.environ["RUNNER_TEMP"]) / "api-scanner-qualification.json"))
