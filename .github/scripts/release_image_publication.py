"""Publish the exact OCI archive accepted by the image-secret scanner."""
import hashlib
import os
import re
import subprocess
import sys
from pathlib import Path

from image_secret_gate import ScanRefusal, scan_archive
from deployables import name as deployable_name


class Refusal(Exception):
    """Safe publication diagnostics; tool output may contain image content."""


def require(condition, message):
    if not condition:
        raise Refusal(message)


def command(*args):
    try:
        result = subprocess.run(args, capture_output=True, text=True, timeout=900, check=False)
    except (OSError, subprocess.TimeoutExpired) as error:
        raise Refusal("Image publication command could not complete") from error
    require(result.returncode == 0, "Image publication command failed; output withheld")
    return result.stdout


def checksum(path):
    result = hashlib.sha256()
    with path.open("rb") as stream:
        for block in iter(lambda: stream.read(1024 * 1024), b""):
            result.update(block)
    return result.hexdigest()


def publish_archive(archive, component, version, revision, repository, scanner, run_command=command):
    require(component in ("api", "web", "runtime", "cmaeg", "runtime-omarchy"), "Unknown release component")
    require(re.fullmatch(r"v[0-9]+\.[0-9]+\.[0-9]+", version), "Invalid release version")
    require(re.fullmatch(r"[0-9a-f]{40}", revision), "A full source revision is required")
    require(re.fullmatch(r"ghcr\.io/[a-z0-9]+(?:[._-][a-z0-9]+)*/" + deployable_name(component), repository),
            "Release repository does not match its component")
    output = Path(os.environ["SKOPEO_OUTPUT_DIR"]).resolve()
    require(archive.resolve() == output / "candidate.oci.tar" and archive.is_file()
            and not archive.is_symlink() and archive.stat().st_size > 0, "Missing or misplaced release archive")
    skopeo = os.environ["SKOPEO_COMMAND"]
    authfile = os.environ["SKOPEO_AUTHFILE"]
    original = checksum(archive)
    # The scanner traverses every platform and every layer in this archive.
    # Publication must not rebuild, flatten, or substitute the scanned bytes.
    scanner(archive, output / "secret-scan", component=component)
    require(checksum(archive) == original, "Release archive changed during its secret scan")
    run_command(str(Path(__file__).resolve().parents[1] / "assert-release-image-tags-absent.sh"),
                component, version, revision, repository)
    manifest = run_command(skopeo, "inspect", "--raw", "oci-archive:/output/candidate.oci.tar")
    require(isinstance(manifest, str) and bool(manifest), "Scanned archive manifest is unavailable")
    for tag in (version, revision[:7]):
        require(checksum(archive) == original, "Release archive changed before publication")
        run_command(skopeo, "copy", "--all", "--preserve-digests", "--authfile", authfile,
                    "oci-archive:/output/candidate.oci.tar", f"docker://{repository}:{tag}")
        remote = run_command(skopeo, "inspect", "--raw", "--authfile", authfile, f"docker://{repository}:{tag}")
        require(remote == manifest, "Published release manifest differs from the scanned image")
    print(f"Published scanned {component} OCI archive sha256:{original} to {version} and {revision[:7]}")
    return original


def main(argv=None):
    args = sys.argv[1:] if argv is None else argv
    try:
        require(len(args) == 5, "Expected archive, component, version, full revision and repository")
        archive, component, version, revision, repository = args
        publish_archive(Path(archive), component, version, revision, repository, scanner=scan_archive)
    except (Refusal, ScanRefusal) as error:
        print(f"::error::{error}")
        return 1
    except (OSError, ValueError, KeyError, TypeError):
        print("::error::Invalid image publication context; details withheld")
        return 1
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
