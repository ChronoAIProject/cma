#!/usr/bin/env python3
"""Bridge only API Cargo mounts; cached objects never replace image acceptance."""
import datetime
import hashlib
import json
import os
from pathlib import Path, PurePosixPath
import re
import shutil
import subprocess
import sys
import tarfile
import tempfile
import uuid


ROOT = Path(__file__).resolve().parents[2]
REPOSITORY = "ChronoAIProject/cma"
FORMAT = 1
# The same Alpine 3.22 index already used by the API image, pinned for this helper.
BRIDGE_IMAGE = "alpine:3.22@sha256:5291449c3df73caf6ed85e649dec1b9e818b39a5d8c871e97afc13e9cd5e8fa8"
MOUNTS = {"registry": "cma-api-registry", "target": "cma-api-target-amd64"}
MAX_ARCHIVE_BYTES = 1024**3
MAX_CONTENT_BYTES = 4 * 1024**3
MAX_MEMBERS = 200_000


def require(value, message):
    if not value:
        raise ValueError(message)


def digest(path):
    result = hashlib.sha256()
    with path.open("rb") as source:
        for block in iter(lambda: source.read(1024 * 1024), b""):
            result.update(block)
    return result.hexdigest()


def authority(env, event):
    require(env.get("GITHUB_REPOSITORY") == REPOSITORY, "repository differs")
    require(event.get("repository", {}).get("full_name") == REPOSITORY, "event repository differs")
    require(re.fullmatch(r"[0-9a-f]{40}", env.get("GITHUB_SHA", "")), "full source SHA required")
    if env.get("GITHUB_EVENT_NAME") == "push":
        require(env.get("GITHUB_REF") == "refs/heads/develop", "only develop push can seed the branch cache")
    else:
        require(env.get("GITHUB_EVENT_NAME") == "pull_request", "unsupported cache event")
        request = event.get("pull_request", {})
        require(request.get("head", {}).get("repo", {}).get("full_name") == REPOSITORY, "fork cache disabled")
        require(request.get("base", {}).get("repo", {}).get("full_name") == REPOSITORY, "PR base repository differs")
        require(request.get("base", {}).get("ref") in ("develop", "main"), "PR base branch differs")
        number = event.get("number")
        require(type(number) is int and number > 0, "PR number required")
        require(env.get("GITHUB_REF") == f"refs/pull/{number}/merge", "PR merge ref required")


def compatibility(root, now):
    paths = {root / "infra/api/Dockerfile", root / "api/Cargo.lock", root / "api/rust-toolchain.toml",
             root / ".github/scripts/api-cargo-cache.py"}
    paths.add(root / "api/Cargo.toml")
    paths.update((root / "api/crates").glob("*/Cargo.toml"))
    for directory in (root / ".cargo", root / "api/.cargo"):
        if directory.is_dir():
            paths.update(path for path in directory.rglob("*") if path.is_file())
    result = hashlib.sha256()
    for path in sorted(paths):
        require(path.is_file() and not path.is_symlink(), "cache inputs must be regular tracked files")
        result.update(path.relative_to(root).as_posix().encode() + b"\0" + path.read_bytes() + b"\0")
    # Bounded refresh even if an upstream base tag or package repository changes.
    year, week, _ = now.isocalendar()
    return f"cma-api-cargo-v{FORMAT}-linux-amd64-musl-{year}w{week:02d}-{result.hexdigest()}-"


def emit(name, value):
    with Path(os.environ["GITHUB_OUTPUT"]).open("a") as output:
        output.write(f"{name}={value}\n")


def context():
    authority(os.environ, json.loads(Path(os.environ["GITHUB_EVENT_PATH"]).read_text()))
    temporary = Path(os.environ["RUNNER_TEMP"]).resolve(strict=True)
    require(temporary.is_dir(), "runner temporary directory required")
    directory = temporary / "cma-api-cargo-cache"
    require(not directory.is_symlink(), "cache directory cannot be a symlink")
    return directory


def prepare(directory):
    directory.mkdir(mode=0o700, exist_ok=False)
    prefix = compatibility(ROOT, datetime.datetime.now(datetime.timezone.utc))
    state = {"prefix": prefix, "key": prefix + os.environ["GITHUB_SHA"]}
    (directory / "state.json").write_text(json.dumps(state))
    for name, value in state.items():
        emit(name, value)


def archive_path(name):
    require(not name.startswith("/"), "absolute archive member")
    path = PurePosixPath(name)
    require(".." not in path.parts, "archive traversal")
    return path


def validate_bundle(directory, prefix):
    require(directory.is_dir() and not directory.is_symlink(), "cache bundle absent")
    require({path.name for path in directory.iterdir()} == {"manifest.json", "registry.tar.gz", "target.tar.gz"},
            "unexpected cache files")
    manifest_path = directory / "manifest.json"
    require(not manifest_path.is_symlink() and manifest_path.stat().st_size < 4096, "invalid cache manifest")
    manifest = json.loads(manifest_path.read_text())
    require(isinstance(manifest, dict), "cache manifest must be an object")
    require(manifest.get("format") == FORMAT and manifest.get("prefix") == prefix, "incompatible cache bundle")
    archives = manifest.get("archives")
    require(isinstance(archives, dict) and set(archives) == set(MOUNTS), "missing cache mount")
    size = sum((directory / f"{name}.tar.gz").lstat().st_size for name in MOUNTS)
    require(size <= MAX_ARCHIVE_BYTES, "compressed cache exceeds 1 GiB")
    content = count = 0
    for name in MOUNTS:
        path = directory / f"{name}.tar.gz"
        require(path.is_file() and not path.is_symlink(), "archive must be a regular file")
        require(manifest["archives"][name] == digest(path), "archive checksum differs")
        files = 0
        with tarfile.open(path, "r|gz") as archive:
            for member in archive:
                require(member.size >= 0, "negative archive member size")
                count += 1
                content += member.size
                require(count <= MAX_MEMBERS and content <= MAX_CONTENT_BYTES, "expanded cache exceeds bounds")
                archive_path(member.name)
                require(member.isdir() or member.isfile() or member.islnk(), "cache contains a special file or symlink")
                files += int(member.isfile())
                if member.islnk():
                    archive_path(member.linkname)
        require(files > 0, "empty mount cannot replace a warm cache")
    return manifest


def dockerfile(operation):
    mounts = " ".join(f"--mount=type=cache,id={mount},target=/cache/{name},sharing=locked"
                      for name, mount in MOUNTS.items())
    # IgnoreCache/--no-cache can prune the very cache mounts being transferred.
    # A unique RUN environment invalidates the bridge layer without that flag.
    header = f"FROM {BRIDGE_IMAGE} AS transfer\nARG CMA_CACHE_TRANSFER\n"
    if operation == "inject":
        return header + "COPY registry.tar.gz target.tar.gz /restore/\n" + f"RUN --network=none {mounts} " + (
            "set -eu; test -n \"$CMA_CACHE_TRANSFER\"; tar -xzpf /restore/registry.tar.gz -C /cache/registry; "
            "tar -xzpf /restore/target.tar.gz -C /cache/target\n")
    require(operation == "extract", "unknown cache transfer")
    return header + f"RUN --network=none {mounts} " + (
        "set -euo pipefail; test -n \"$CMA_CACHE_TRANSFER\"; mkdir /out; "
        f"total=$(du -sk /cache/registry /cache/target | awk '{{n += $1}} END {{print n}}'); "
        f"if [ \"$total\" -gt {MAX_CONTENT_BYTES // 1024} ]; then touch /out/oversize; exit 0; fi; "
        "tar -C /cache/registry -cf - . | gzip -1 > /out/registry.tar.gz; "
        "tar -C /cache/target -cf - . | gzip -1 > /out/target.tar.gz\n"
        "FROM scratch\nCOPY --from=transfer /out/ /\n")


def transfer(operation, directory, builder=None):
    command = ["docker", "buildx", "build", "--platform", "linux/amd64", "--progress=plain",
               "--build-arg", f"CMA_CACHE_TRANSFER={uuid.uuid4().hex}"]
    if builder:
        command += ["--builder", builder]
    if operation == "inject":
        command += ["--output", "type=cacheonly", "--file", "-", str(directory)]
        subprocess.run(command, input=dockerfile(operation), text=True, check=True, timeout=150)
    else:
        with tempfile.TemporaryDirectory(prefix="cma-cache-context-") as empty:
            command += ["--output", f"type=local,dest={directory}", "--file", "-", empty]
            subprocess.run(command, input=dockerfile(operation), text=True, check=True, timeout=150)


def seal(directory, prefix):
    require(not (directory / "oversize").exists(), "expanded cache exceeds 4 GiB")
    manifest = {"format": FORMAT, "prefix": prefix,
                "archives": {name: digest(directory / f"{name}.tar.gz") for name in MOUNTS}}
    (directory / "manifest.json").write_text(json.dumps(manifest))
    validate_bundle(directory, prefix)


def run(operation, directory):
    prefix = json.loads((directory / "state.json").read_text())["prefix"]
    bundle = directory / "data"
    if operation == "inject":
        if not bundle.exists():
            print("API Cargo cache miss; build starts cold.")
            return
        validate_bundle(bundle, prefix)
        transfer("inject", bundle)
        print("Restored API Cargo mounts; all image checks still run.")
    else:
        require(operation == "extract", "unknown cache command")
        # The restored bundle is expendable. Export while the BuildKit mounts
        # still exist, before CI's mandatory buildx prune and image scan.
        if bundle.exists():
            require(not bundle.is_symlink(), "cache directory cannot be a symlink")
            shutil.rmtree(bundle)
        transfer("extract", bundle)
        seal(bundle, prefix)
        emit("ready", "true")


def main():
    require(len(sys.argv) == 2 and sys.argv[1] in ("prepare", "inject", "extract"), "expected prepare, inject or extract")
    directory = context()  # Authority failures are errors, never cache misses.
    operation = sys.argv[1]
    if operation == "prepare":
        prepare(directory)
        return
    if operation == "extract":
        emit("ready", "false")
    try:
        run(operation, directory)
    except (ValueError, KeyError, EOFError, OSError, tarfile.TarError, subprocess.SubprocessError) as error:
        # Cache availability cannot replace or skip the real build and scan.
        print(f"::warning::API Cargo cache {operation} unavailable ({type(error).__name__}); normal image checks remain required.")


if __name__ == "__main__":
    main()
