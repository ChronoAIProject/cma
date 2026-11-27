"""Generate the AM dependency cache on a runner detached from publication."""
import json
import os
import secrets
import subprocess
from pathlib import Path
from tempfile import TemporaryDirectory

from actions_layer_admission import source_run
from buildkit_cache_archive import seal
from staging_cd_build import command, identity, snapshot
from staging_cd_common import require
from staging_cd_layer_reuse import seal_replay
from staging_cd_receipts import validate_receipt
from staging_cd_reservations import validate_reservation


def reservation_for(path, run):
    require(path.is_file() and not path.is_symlink() and path.stat().st_size <= 1024**2,
            "AM publication receipt is unavailable")
    receipts = json.loads(path.read_text())
    require(isinstance(receipts, list) and receipts, "AM publication receipts are invalid")
    for receipt in receipts:
        validate_receipt(receipt)
    receipt = receipts[-1]
    release = receipt["identity"]
    require((release["component"], release["run_id"], release["run_attempt"]) ==
            ("am", run["id"], run["run_attempt"]), "AM publication identity differs")
    reservation = dict(release, schema="cma-version-reservation-v1")
    del reservation["build"]
    return validate_reservation(reservation), release


def export(root, parent, run, *, runner=command):
    reservation, release = reservation_for(parent / "receipts.json", run)
    source_sha = reservation["source"]
    tree = runner(["git", "rev-parse", source_sha + "^{tree}"], cwd=root, timeout=30).decode().strip()
    require(tree == reservation["tree"], "AM source tree differs from publication")
    runner(["git", "merge-base", "--is-ancestor", source_sha, run["head_sha"]], cwd=root, timeout=30)
    archive = parent / "archive.tar"
    replay = parent / "replay.json"
    require(not archive.exists() and not replay.exists(), "AM layer candidate must be fresh")
    namespace = secrets.token_hex(16)
    with TemporaryDirectory(prefix="am-dependencies-", dir=parent) as directory:
        work = Path(directory)
        source = work / "source"
        snapshot(root, source_sha, source, runner=runner)
        reconstructed, recipe, arguments = identity(reservation, source)
        require(reconstructed == release, "AM build recipe differs from qualified publication")
        layers = work / "layers"
        args = ["docker", "buildx", "build", "--progress=rawjson", "--platform", "linux/amd64",
                "--file", str(source / recipe["dockerfile"]), "--target", "dependencies",
                "--output", "type=cacheonly", "--cache-to",
                f"type=local,mode=min,image-manifest=true,oci-mediatypes=true,dest={layers}",
                "--build-arg", "BUILDKIT_CACHE_MOUNT_NS=" + namespace]
        for name, value in sorted(arguments.items()):
            args += ["--build-arg", name + "=" + value]
        runner([*args, str(source)], timeout=1800)
        seal(layers, archive)
    replay.write_text(json.dumps({"reservation": reservation, "mount_namespace": namespace,
                                  "kind": "am-dependencies"}))
    seal_replay(replay, archive)


def main():
    env = os.environ
    run = source_run(env)
    require(env.get("CD_COMPONENT") == "am", "AM dependency archive component differs")
    export(Path.cwd(), Path(env["RUNNER_TEMP"]) / "cma-layer-cache", run)


if __name__ == "__main__":
    try:
        main()
    except (OSError, ValueError, KeyError, TypeError, subprocess.SubprocessError) as error:
        raise SystemExit(f"AM dependency archive withheld ({type(error).__name__})") from None
