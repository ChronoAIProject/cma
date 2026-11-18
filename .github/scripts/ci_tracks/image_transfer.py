"""Bind the native image between build and scan jobs without rebuilding it."""

import hashlib
import importlib.util
import json
import os
from pathlib import Path
import re
import subprocess
import sys

from recipe import resolve

MAX_ARCHIVE = 8 * 1024 * 1024 * 1024


def require(condition):
    if not condition:
        raise ValueError("image transfer identity or integrity differs")


def exporter():
    path = Path(__file__).resolve().parents[1] / "export-ci-native-image.py"
    spec = importlib.util.spec_from_file_location("native_image_export", path)
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


def identity(environment):
    component = environment["IMAGE_COMPONENT"]
    name = resolve(Path(__file__).resolve().parents[3], component)["name"]
    revision = environment["GITHUB_SHA"]
    require(environment["IMAGE_NAME"] == name and re.fullmatch(r"[a-f0-9]{40}", revision))
    return component, name, revision


def validate(metadata, directory, environment):
    component, name, revision = identity(environment)
    require(metadata["schema"] == "cma-ci-build-image-v1"
            and metadata["component"] == component and metadata["sourceRevision"] == revision
            and metadata["testedReference"] == name + ":ci" and metadata["platform"] == "linux/amd64")
    for field, key in (("repository", "GITHUB_REPOSITORY"), ("runId", "GITHUB_RUN_ID"),
                       ("runAttempt", "GITHUB_RUN_ATTEMPT"), ("event", "GITHUB_EVENT_NAME")):
        require(metadata["workflow"][field] == environment[key])
    require(re.fullmatch(r"sha256:[a-f0-9]{64}", metadata["imageId"]))
    require(metadata["archive"] == f"cma-{component}-linux-amd64.docker.tar.gz")
    archive = directory / metadata["archive"]
    require(not archive.is_symlink() and archive.is_file())
    require(type(metadata["archiveBytes"]) is int and 0 < metadata["archiveBytes"] <= MAX_ARCHIVE
            and archive.stat().st_size == metadata["archiveBytes"])
    with archive.open("rb") as stream:
        require(hashlib.file_digest(stream, "sha256").hexdigest() == metadata["archiveSha256"])
    return archive


def save(directory, environment):
    component, _, revision = identity(environment)
    exporter().export(component, revision, directory)
    path = directory / "metadata.json"
    metadata = json.loads(path.read_text())
    # An intermediate build is not a tested/publication artifact. The existing
    # tested-image exporter runs again only after scan and product proofs pass.
    metadata["schema"] = "cma-ci-build-image-v1"
    path.write_text(json.dumps(metadata, indent=2) + "\n")
    validate(metadata, directory, environment)


def load(directory, environment):
    path = directory / "metadata.json"
    require(not path.is_symlink() and path.stat().st_size <= 16384)
    metadata = json.loads(path.read_text())
    archive = validate(metadata, directory, environment)
    subprocess.run(["docker", "image", "load", "--input", str(archive)],
                   check=True, capture_output=True, timeout=600)
    # Docker-save exports the immutable ID. Restore the expected local tag only
    # after checking the archive, then verify the imported ID and source labels.
    inspected = json.loads(subprocess.check_output(
        ["docker", "image", "inspect", metadata["imageId"]], text=True, timeout=30))
    require(isinstance(inspected, list) and len(inspected) == 1)
    image = inspected[0]
    require(image["Id"] == metadata["imageId"] and image["Os"] == "linux"
            and image["Architecture"] == "amd64"
            and image["Config"]["Labels"]["org.opencontainers.image.revision"] == environment["GITHUB_SHA"])
    subprocess.run(["docker", "image", "tag", metadata["imageId"], metadata["testedReference"]],
                   check=True, capture_output=True, timeout=30)


def main():
    try:
        action = {"save": save, "load": load}[sys.argv[1]]
        action(Path(os.environ["RUNNER_TEMP"]) / "ci-build-image", os.environ)
    except (IndexError, KeyError, OSError, TypeError, ValueError, subprocess.SubprocessError):
        print("::error::Native image transfer failed identity or integrity validation")
        return 1
    print("Native image transfer identity and integrity verified")
    return 0


if __name__ == "__main__":
    sys.exit(main())
