"""Prove that a clean builder can reuse the exact accepted layer archive."""

import json
import os
import re
import secrets
import subprocess
import sys
from pathlib import Path
from tempfile import TemporaryDirectory

from buildkit_cache_archive import digest, inventory, MAX_BYTES
from staging_cd_build import build
from staging_cd_common import require
from staging_cd_reservations import validate_reservation


MAX_REPLAY_BYTES = 4096


def read_replay(path, *, sealed):
    require(path.is_file() and not path.is_symlink() and 0 < path.stat().st_size <= MAX_REPLAY_BYTES,
            "Layer replay identity is unavailable")
    value = json.loads(path.read_text())
    fields = {"reservation", "mount_namespace", "archive_digest"} if sealed else {"reservation", "mount_namespace"}
    require(isinstance(value, dict) and set(value) in (fields, fields | {"kind"}), "Layer replay identity differs")
    validate_reservation(value["reservation"])
    if "kind" in value:
        require(value["kind"] == "am-dependencies" and value["reservation"]["component"] == "am",
                "Layer replay kind differs")
    mount = value["mount_namespace"]
    require(mount is None or isinstance(mount, str) and re.fullmatch(r"[a-f0-9]{32}", mount),
            "Layer replay mount identity differs")
    return value


def seal_replay(path, archive):
    value = read_replay(path, sealed=False)
    with archive.open("rb") as source:
        value["archive_digest"] = digest(source)
    path.write_text(json.dumps(value))


class ReplayCache:
    def __init__(self, root, mount):
        self.root, self.mount = root, mount

    def build_options(self):
        args = ["--cache-from", "type=local,src=" + str(self.root)]
        if self.mount is not None:
            args += ["--build-arg", "BUILDKIT_CACHE_MOUNT_NS=" + self.mount]
        return args

    def build_environment(self):
        return {}

    def record_layer_replay(self, reservation):
        # The replay never exports a cache.
        pass


def reused(progress):
    return (progress.get("complete") is True and progress.get("local_cache_imports") == 1
            and type(progress.get("cached_build_steps")) is int and progress["cached_build_steps"] > 0
            and not progress.get("failed_steps"))


def prove(root, archive, replay, env, *, builder=build):
    from actions_layer_admission import source_run
    source = source_run(env)
    value = read_replay(replay, sealed=True)
    reservation = value["reservation"]
    require((reservation["component"], reservation["run_id"], reservation["run_attempt"])
            == (env["CD_COMPONENT"], source["id"], source["run_attempt"]), "Layer replay producer differs")
    require(0 < archive.stat().st_size <= MAX_BYTES, "Layer archive exceeds its bound")
    with archive.open("rb") as stream:
        fingerprint = digest(stream)
    require(value["archive_digest"] == fingerprint, "Layer replay archive identity differs")
    inventory(root)
    # The exact publication may have reserved an older develop source. Only
    # replay a tree in that trusted producer's ancestry and verify its digest.
    tree = subprocess.run(["git", "rev-parse", reservation["source"] + "^{tree}"], capture_output=True,
                          text=True, timeout=30, check=True).stdout.strip()
    require(tree == reservation["tree"], "Layer replay tree differs")
    subprocess.run(["git", "merge-base", "--is-ancestor", reservation["source"], source["head_sha"]],
                   capture_output=True, timeout=30, check=True)
    progress = {}
    with TemporaryDirectory(prefix="layer-replay-", dir=env["RUNNER_TEMP"]) as directory:
        mount = secrets.token_hex(16) if value.get("kind") == "am-dependencies" else value["mount_namespace"]
        builder(Path.cwd(), reservation, Path(directory), cache=ReplayCache(root, mount),
                progress=progress)
    require(reused(progress), "Clean builder did not reuse a result from the restored cache")
    if value.get("kind") == "am-dependencies":
        require(progress.get("cached_dependency_steps", 0) > 0, "Clean builder did not reuse AM dependencies")
    return fingerprint


def main(env, *, candidate=False):
    parent = Path(env["RUNNER_TEMP"]) / "cma-layer-cache"
    fingerprint = prove(parent / ("validated" if candidate else "verified"), parent / "archive.tar", parent / "replay.json", env)
    with Path(env["GITHUB_OUTPUT"]).open("a") as output:
        output.write(f"archive_digest={fingerprint}\n")


if __name__ == "__main__":
    try:
        import argparse
        parser = argparse.ArgumentParser(description=__doc__)
        parser.add_argument("--candidate", action="store_true")
        main(os.environ, candidate=parser.parse_args().candidate)
    except Exception as error:
        print(f"Layer reuse proof withheld ({type(error).__name__})", file=sys.stderr)
        sys.exit(1)
