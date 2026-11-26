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

from staging_cd_cache_compaction import export_script


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
    elif env.get("GITHUB_EVENT_NAME") == "merge_group":
        # Queue entries read develop's archive; saving remains develop-push only.
        group = event.get("merge_group", {})
        require(group.get("base_ref") == "refs/heads/develop", "merge group base branch differs")
        require(re.fullmatch(r"refs/heads/gh-readonly-queue/develop/pr-[1-9][0-9]*-[0-9a-f]{40}", env.get("GITHUB_REF", ""))
                and group.get("head_ref") == env["GITHUB_REF"], "develop merge group ref required")
        require(group.get("head_sha") == env["GITHUB_SHA"], "merge group commit differs")
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
    paths = {root / "api/crates/cma/Dockerfile", root / "api/Cargo.lock", root / "api/rust-toolchain.toml",
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


def validate_incremental_member(member, path):
    # Cargo also writes these two bookkeeping files at the target root.
    if path[:2] != ("release", "incremental"):
        require((member.isdir() and path in ((), ("release",))) or
                (member.isfile() and path in ((".rustc_info.json",), ("CACHEDIR.TAG",))),
                "non-incremental entry in Cargo target archive")
    if member.islnk():
        require(archive_path(member.linkname).parts[:2] == ("release", "incremental"),
                "incompatible incremental hard link")


def validate_bundle(directory, prefix, *, family=None, max_archive=None, max_content=None):
    require(directory.is_dir() and not directory.is_symlink(), "cache bundle absent")
    require({path.name for path in directory.iterdir()} == {"manifest.json", "registry.tar.gz", "target.tar.gz"},
            "unexpected cache files")
    manifest_path = directory / "manifest.json"
    require(not manifest_path.is_symlink() and manifest_path.stat().st_size < 4096, "invalid cache manifest")
    manifest = json.loads(manifest_path.read_text())
    require(isinstance(manifest, dict), "cache manifest must be an object")
    stored = manifest.get("prefix")
    if stored != prefix:
        # A graph fallback may seed rustc incremental sessions, never final
        # binaries, dependency objects or previous build-script output.
        allowed = re.escape(family) + r"[0-9a-f]{64}-" if family else None
        require(allowed and isinstance(stored, str) and re.fullmatch(allowed, stored)
                and re.fullmatch(allowed, prefix), "incompatible cache bundle")
    require(manifest.get("format") == FORMAT, "incompatible cache format")
    archives = manifest.get("archives")
    require(isinstance(archives, dict) and set(archives) == set(MOUNTS), "missing cache mount")
    size = sum((directory / f"{name}.tar.gz").lstat().st_size for name in MOUNTS)
    # The Actions cache path keeps MAX_ARCHIVE_BYTES; staging run artifacts pass their own bound.
    require(size <= (MAX_ARCHIVE_BYTES if max_archive is None else max_archive),
            "compressed cache exceeds its bound")
    content = count = 0
    expanded = MAX_CONTENT_BYTES if max_content is None else max_content
    for name in MOUNTS:
        path = directory / f"{name}.tar.gz"
        require(path.is_file() and not path.is_symlink(), "archive must be a regular file")
        require(manifest["archives"][name] == digest(path), "archive checksum differs")
        files = 0
        regular_members = set()
        target_links = set()
        with tarfile.open(path, "r|gz") as archive:
            for member in archive:
                require(member.size >= 0, "negative archive member size")
                count += 1
                content += member.size
                require(count <= MAX_MEMBERS and content <= expanded, "expanded cache exceeds bounds")
                member_path = archive_path(member.name)
                if family is not None and name == "target":
                    validate_incremental_member(member, member_path.parts)
                require(member.isdir() or member.isfile() or member.islnk(), "cache contains a special file or symlink")
                files += int(member.isfile())
                if family is not None and name == "target":
                    if member.isfile():
                        regular_members.add(member_path)
                    elif member.islnk():
                        target_links.add(archive_path(member.linkname))
                if member.islnk():
                    archive_path(member.linkname)
        require(target_links <= regular_members, "cache hard link target is absent")
        require(files > 0, "empty mount cannot replace a warm cache")
    return manifest


def dockerfile(operation, max_content=None, *, compact_registry=False, max_archive=None):
    names = ("target",) if operation == "inject-target" else tuple(MOUNTS)
    mounts = " ".join(f"--mount=type=cache,id={MOUNTS[name]},target=/cache/{name},sharing=locked"
                      for name in names)
    # IgnoreCache/--no-cache can prune the very cache mounts being transferred.
    # A unique RUN environment invalidates the bridge layer without that flag.
    header = f"FROM {BRIDGE_IMAGE} AS transfer\nARG CMA_CACHE_TRANSFER\n"
    if operation in ("inject", "inject-target"):
        # The compressed inputs are needed only while tar populates the
        # mounts. Keep them out of BuildKit's intermediate image layers.
        archives = " ".join(f"--mount=type=bind,source={name}.tar.gz,target=/restore/{name}.tar.gz"
                            for name in names)
        commands = "; ".join(f"tar -xzpf /restore/{name}.tar.gz -C /cache/{name}" for name in names)
        return (header + f"RUN --network=none {mounts} {archives} "
                + f'set -eu; test -n "$CMA_CACHE_TRANSFER"; {commands}\n')
    require(operation == "extract", "unknown cache transfer")
    bound_kib = (MAX_CONTENT_BYTES if max_content is None else max_content) // 1024
    if compact_registry:
        script = export_script(
            MAX_CONTENT_BYTES if max_content is None else max_content,
            MAX_ARCHIVE_BYTES if max_archive is None else max_archive,
            MAX_MEMBERS,
        )
        return (header + f"RUN --network=none {mounts} <<'CACHE_EXPORT'\n"
                + 'set -eu\nset -- /cache/registry /cache/target /out\n'
                + 'test -n "$CMA_CACHE_TRANSFER"\n' + script
                + "CACHE_EXPORT\nFROM scratch\nCOPY --from=transfer /out/ /\n")
    return header + f"RUN --network=none {mounts} " + (
        'set -euo pipefail; test -n "$CMA_CACHE_TRANSFER"; mkdir /out; '
        "total=$(du -sk /cache/registry /cache/target | awk '{n += $1} END {print n}'); skip_src=; "
        f'if [ "$total" -gt {bound_kib} ]; then touch /out/oversize; exit 0; fi; '
        "tar -C /cache/registry -cf - $skip_src . | gzip -1 > /out/registry.tar.gz; "
        "tar -C /cache/target -cf - . | gzip -1 > /out/target.tar.gz\n"
        "FROM scratch\nCOPY --from=transfer /out/ /\n")


def transfer_command(builder, namespace):
    env = {key: value for key, value in os.environ.items()
           if key in ("PATH", "HOME", "LANG", "LC_ALL", "DOCKER_HOST", "DOCKER_CONFIG", "BUILDX_BUILDER")}
    command = ["docker", "buildx", "build", "--platform", "linux/amd64", "--progress=plain",
               "--build-arg", f"CMA_CACHE_TRANSFER={uuid.uuid4().hex}"]
    if namespace is not None:
        command += ["--build-arg", "BUILDKIT_CACHE_MOUNT_NS=" + namespace]
    if builder:
        command += ["--builder", builder]
    return command, env


def transfer(operation, directory, builder=None, *, namespace=None, timeout=150, max_content=None,
             compact_registry=False, max_archive=None):
    command, env = transfer_command(builder, namespace)
    if operation in ("inject", "inject-target"):
        command += ["--output", "type=cacheonly", "--file", "-", str(directory)]
        subprocess.run(command, input=dockerfile(operation, max_content), text=True, check=True,
                       timeout=timeout, env=env)
    else:
        with tempfile.TemporaryDirectory(prefix="cma-cache-context-") as empty:
            command += ["--output", f"type=local,dest={directory}", "--file", "-", empty]
            subprocess.run(command, input=dockerfile(operation, max_content, compact_registry=compact_registry, max_archive=max_archive),
                           text=True, check=True, timeout=timeout, env=env)


def start_extract(directory, context, *, namespace, max_content, max_archive):
    """Start the bounded staging export in a process group owned by the publisher."""
    context.mkdir(mode=0o700)
    recipe = context / "Dockerfile"
    recipe.write_text(dockerfile("extract", max_content, compact_registry=True, max_archive=max_archive))
    command, env = transfer_command(None, namespace)
    command += ["--output", f"type=local,dest={directory}", "--file", str(recipe), str(context)]
    return subprocess.Popen(command, env=env, stdin=subprocess.DEVNULL, stdout=subprocess.DEVNULL,
                            stderr=subprocess.DEVNULL, start_new_session=True)


def seal(directory, prefix, *, family=None, max_archive=None, max_content=None):
    require(not (directory / "oversize").exists(), "expanded cache exceeds its bound")
    require(not (directory / "oversize-compressed").exists(), "compressed cache exceeds its bound")
    require(not (directory / "empty").exists(), "empty mount cannot replace a warm cache")
    manifest = {"format": FORMAT, "prefix": prefix,
                "archives": {name: digest(directory / f"{name}.tar.gz") for name in MOUNTS}}
    (directory / "manifest.json").write_text(json.dumps(manifest))
    validate_bundle(directory, prefix, family=family, max_archive=max_archive, max_content=max_content)


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
