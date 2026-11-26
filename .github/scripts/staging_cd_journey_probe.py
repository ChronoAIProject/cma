"""Build and publish the independent public-journey runner by image digest."""
import json
import os
from pathlib import Path
import subprocess
import sys
import tempfile

import ci_retry
from image_secret_gate import ScanRefusal, checksum, verified_staging_scan
from staging_cd_build import command, inspect, snapshot
from staging_cd_common import matches, require
from staging_cd_receipts import IDENTITY_LABEL, canonical
from staging_cd_registry import JOURNEY_REPOSITORY, Registry
from staging_cd_runtime import run_private

COMPONENT = "journey-probe"
DOCKERFILE = "api/crates/cma-image-manager/Dockerfile"


def identity(root, source):
    require(matches(r"[0-9a-f]{40}", source), "Journey source must be an exact Git revision")
    head = command(["git", "rev-parse", "HEAD"], cwd=root).decode().strip()
    require(head == source, "Journey checkout differs from the push revision")
    tree = command(["git", "rev-parse", "HEAD^{tree}"], cwd=root).decode().strip()
    return dict(schema="cma-journey-image-v1", component=COMPONENT, source=source, version=source, tree=tree)


def build(root, directory, release):
    source = directory / "source"
    archive = directory / "image.oci.tar"
    snapshot(root, release["source"], source)
    subprocess.run([
        "docker", "buildx", "build", "--progress=plain", "--platform", "linux/amd64",
        "--provenance=false", "--sbom=false", "--target", "journey-probe",
        "--file", str(source / DOCKERFILE),
        "--build-arg", "CHRONO_BUILD_SHA=" + release["source"],
        "--label", IDENTITY_LABEL + "=" + canonical(release),
        "--output", "type=oci,dest=" + str(archive), str(source),
    ], check=True, timeout=2700)
    require(archive.is_file(), "Journey build produced no OCI archive")


def publish(directory, release, registry, credential, actor, *, runner=command, scanner=verified_staging_scan):
    archive = directory / "image.oci.tar"
    digest = inspect(archive, release, runner)
    with tempfile.TemporaryDirectory(prefix="journey-publish-", dir=directory) as scratch:
        scratch = Path(scratch)
        scan = scratch / "scan"
        try:
            scanned, (version, bundle) = scanner(archive, scan, component=COMPONENT)
        except ScanRefusal as error:
            if error.evidence is not None:
                (directory / "scan-failure.json").write_text(canonical(error.evidence) + "\n")
            raise
        counts = json.loads((scan / "REPORT.json").read_text())["counts"]
        require(checksum(archive) == scanned, "Journey archive differs from the scanned bytes")

        # Each tag names its own manifest digest, so a retry copies the same bytes
        # and two distinct builds of one source cannot replace each other.
        tag = digest.replace(":", "-", 1)
        auth = scratch / "auth.json"
        runner(["skopeo", "login", "--authfile", str(auth), "--username", actor, "--password-stdin", "ghcr.io"],
               data=credential.encode(), timeout=60)
        auth.chmod(0o600)
        ci_retry.call(
            lambda: runner(["skopeo", "copy", "--preserve-digests", "--authfile", str(auth),
                            "oci-archive:" + str(archive), "docker://" + JOURNEY_REPOSITORY + ":" + tag],
                           timeout=600),
            name="journey-registry-copy", classify=ci_retry.classify, attempts=2,
        )
        remote = registry.manifest(JOURNEY_REPOSITORY, tag, settle=True)
        require(remote[0] == digest and registry.exists(JOURNEY_REPOSITORY, digest, settle=True),
                "GHCR did not retain the scanned journey image digest")
        require(checksum(archive) == scanned, "Journey archive changed during publication")

    receipt = dict(schema="cma-journey-publication-v1", source=release["source"], tree=release["tree"],
                   image=JOURNEY_REPOSITORY + "@" + digest, tag=tag, platform="linux/amd64",
                   archive_sha256=scanned, scan=dict(version=version, bundle_sha256=bundle, counts=counts))
    (directory / "publication.json").write_text(canonical(receipt) + "\n")
    return receipt


def main():
    root = Path.cwd()
    directory = Path(os.environ["RUNNER_TEMP"]) / "journey-image"
    mode, = sys.argv[1:]
    if mode == "build":
        directory.mkdir()
        build(root, directory, identity(root, os.environ["GITHUB_SHA"]))
        return 0

    def operation(mask):
        require(mode == "publish", "Unknown journey image operation")
        credential, actor = os.environ["GH_TOKEN"], os.environ["GITHUB_ACTOR"]
        mask(credential)
        registry = Registry(credential=credential, actor=actor)
        publish(directory, identity(root, os.environ["GITHUB_SHA"]), registry, credential, actor)
        return True

    status = run_private(operation, None, subject="journey-probe-publication")
    if status == 0:
        print((directory / "publication.json").read_text(), end="")
    return status


if __name__ == "__main__":
    raise SystemExit(main())
