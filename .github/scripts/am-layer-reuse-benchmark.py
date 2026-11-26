#!/usr/bin/env python3
"""Measure a bounded layer archive on two fresh hosted AM builders."""

import argparse
from contextlib import contextmanager
from datetime import datetime, timezone
import json
import os
from pathlib import Path
import secrets
import shutil
import subprocess
import sys
import tempfile
import time

from staging_cd_build_progress import summarize

CONTROL = Path(__file__).resolve().parents[2]
CANDIDATE_SHA = "2268efc52d738845d245d3b49b2409e404724d26"
EDIT = "api/crates/cma/src/store/migrations/index_toolkit/mod.rs"
OLD = b'"index-toolkit-catalog-v1"'
NEW = b'"index-toolkit-catalog-layer-reuse"'


def run(args, *, cwd=None, timeout=180, data=None, env=None):
    return subprocess.run(args, cwd=cwd, timeout=timeout, input=data, env=env,
                          capture_output=True, check=True).stdout


def edit(root, index):
    content = run(["git", "show", f"{CANDIDATE_SHA}:{EDIT}"], cwd=root)
    if content.count(OLD) != 1:
        raise ValueError("expected one catalog marker")
    env = dict(os.environ, GIT_INDEX_FILE=str(index), GIT_AUTHOR_NAME="Benchmark",
               GIT_AUTHOR_EMAIL="benchmark@example.com", GIT_COMMITTER_NAME="Benchmark",
               GIT_COMMITTER_EMAIL="benchmark@example.com")
    def git(*args, data=None):
        return run(["git", *args], cwd=root, data=data, env=env).decode().strip()
    blob = git("hash-object", "-w", "--stdin", data=content.replace(OLD, NEW))
    git("read-tree", CANDIDATE_SHA)
    git("update-index", "--cacheinfo", f"100644,{blob},{EDIT}")
    tree = git("write-tree")
    edited = git("commit-tree", tree, "-p", CANDIDATE_SHA, "-m", "Edit synthetic catalogue marker")
    if git("diff", "--name-only", CANDIDATE_SHA, edited) != EDIT:
        raise ValueError("unexpected benchmark edit")
    return edited


@contextmanager
def builder(layers, *, seed):
    name = "am-layer-reuse-" + secrets.token_hex(6)
    namespace = secrets.token_hex(16)

    class Cache:
        timing = None

        def build_options(self):
            options = ["--builder", name, "--build-arg", "BUILDKIT_CACHE_MOUNT_NS=" + namespace]
            if seed:
                return options
            return options + ["--cache-from", f"type=local,src={layers}"]

        def build_environment(self):
            return {}

        def record_layer_replay(self, _reservation):
            pass

        def run(self, args, *, timeout, cwd=None, **_):
            if args[:3] != ["docker", "buildx", "build"]:
                return run(args, cwd=cwd, timeout=timeout)
            start = time.monotonic()
            result = subprocess.run(args, timeout=min(timeout, 4200), capture_output=True, check=True)
            self.timing = summarize(result.stderr.decode(errors="replace"))
            self.timing["seconds"] = round(time.monotonic() - start, 3)
            if not self.timing["complete"]:
                raise ValueError("incomplete BuildKit progress")
            return result.stdout

    # The cache bridge must use the same explicit builder for mount transfer.
    run(["docker", "buildx", "create", "--name", name, "--driver", "docker-container"])
    try:
        yield (Cache(), namespace, name)
    finally:
        run(["docker", "buildx", "rm", name])


def benchmark(root, work, evidence):
    # Use the reviewed opt-in recipe as immutable test input and the current
    # control checkout's bounded layer archive admission and timing parser.
    sys.path.insert(0, str(root / ".github/scripts"))
    from staging_cd_build import build, inspect, snapshot, identity as release_identity
    from staging_cd_build_cache import bridge, MAX_BUNDLE_BYTES, MAX_EXPANDED_BYTES
    from staging_cd_build_cargo_keys import identity
    from staging_cd_reservations import next_reservation

    def git(*args):
        return run(["git", *args], cwd=root).decode().strip()

    if git("rev-parse", "HEAD") != CANDIDATE_SHA:
        raise ValueError("unexpected profile candidate source")
    edited = edit(root, work / "index")
    evidence["library_edit"] = edited

    def reserve(revision, earlier=()):
        return next_reservation("am", revision, git("rev-parse", revision + "^{tree}"), 1, "patch",
                                int(os.environ["GITHUB_RUN_ID"]), int(os.environ["GITHUB_RUN_ATTEMPT"]), [], earlier)

    cold = reserve(CANDIDATE_SHA)
    warm = reserve(edited, [cold])
    for directory, request in ((work / "cold", cold), (work / "warm", warm)):
        directory.mkdir()
        snapshot(root, request["source"], directory / "source")
    day = datetime.now(timezone.utc).date()
    key = identity(work / "cold/source", "am", day=day, staging_am_opt_level="0")
    if identity(work / "warm/source", "am", day=day, staging_am_opt_level="0") != key:
        raise ValueError("benchmark edit changed Cargo compatibility")
    tool = bridge()
    options = {"max_archive": MAX_BUNDLE_BYTES, "max_content": MAX_EXPANDED_BYTES}

    def measured_image(cache, request, directory):
        archive, release = build(root, request, directory, source=directory / "source",
                                 runner=cache.run, cache=cache, staging_am_opt_level="0")
        digest = inspect(archive, release)
        result = dict(cache.timing, digest=digest, source=release["source"])
        return archive, result

    with builder(work / "layers", seed=True) as (cache, namespace, name):
        image, evidence["cold"] = measured_image(cache, cold, work / "cold")
        image.unlink()
        tool.transfer("extract", work / "cargo", name, namespace=namespace,
                      compact_registry=True, timeout=360, **options)
        (work / "cargo/compacted").unlink(missing_ok=True)
        tool.seal(work / "cargo", key.prefix, family=key.family, **options)

    # Matched baseline: same edited tree, Cargo seed, CPU and fresh builder;
    # the optional dependency-layer archive is absent.
    baseline = work / "warm-baseline"
    baseline.mkdir()
    snapshot(root, warm["source"], baseline / "source")
    with builder(work / "unused", seed=True) as (cache, namespace, name):
        start = time.monotonic()
        tool.validate_bundle(work / "cargo", key.prefix, family=key.family, **options)
        tool.transfer("inject", work / "cargo", name, namespace=namespace,
                      timeout=360, max_content=MAX_EXPANDED_BYTES)
        cargo_restore_seconds = time.monotonic() - start
        image, evidence["baseline"] = measured_image(cache, warm, baseline)
        evidence["baseline"]["cargo_restore_seconds"] = round(cargo_restore_seconds, 3)
        evidence["baseline"]["total_seconds"] = round(evidence["baseline"]["seconds"] + cargo_restore_seconds, 3)
        image.unlink()

    # Reconstruct only the dependency target on a fresh, disposable builder.
    # The preceding complete image and Cargo archive have already settled.
    with builder(work / "layers", seed=True) as (cache, namespace, name):
        source = work / "cold/source"
        _, recipe, arguments = release_identity(cold, source, staging_am_opt_level="0")
        args = ["docker", "buildx", "build", "--progress=rawjson", "--platform", "linux/amd64",
                "--builder", name, "--file", str(source / recipe["dockerfile"]), "--target", "dependencies",
                "--output", "type=cacheonly", "--cache-to",
                f"type=local,mode=min,image-manifest=true,oci-mediatypes=true,dest={work / 'layers'}",
                "--build-arg", "BUILDKIT_CACHE_MOUNT_NS=" + namespace]
        for key_name, value in sorted(arguments.items()):
            args += ["--build-arg", key_name + "=" + value]
        cache.run([*args, str(source)], timeout=1800)
        evidence["dependency_reconstruction"] = cache.timing

    validator = CONTROL / ".github/scripts/buildkit_cache_archive.py"
    archive = work / "layers.tar"
    start = time.monotonic()
    run([sys.executable, "-B", str(validator), "seal", str(work / "layers"), str(archive)], timeout=360)
    evidence["layer_bytes"] = archive.stat().st_size
    evidence["seal_seconds"] = round(time.monotonic() - start, 3)
    shutil.rmtree(work / "layers")
    start = time.monotonic()
    run([sys.executable, "-B", str(validator), "restore", str(work / "restored"), str(archive)], timeout=360)
    layer_restore_seconds = time.monotonic() - start
    archive.unlink()
    with builder(work / "restored", seed=False) as (cache, namespace, name):
        start = time.monotonic()
        tool.validate_bundle(work / "cargo", key.prefix, family=key.family, **options)
        tool.transfer("inject", work / "cargo", name, namespace=namespace,
                      timeout=360, max_content=MAX_EXPANDED_BYTES)
        cargo_restore_seconds = time.monotonic() - start
        image, evidence["warm"] = measured_image(cache, warm, work / "warm")
        result = evidence["warm"]
        result["layer_restore_seconds"] = round(layer_restore_seconds, 3)
        result["cargo_restore_seconds"] = round(cargo_restore_seconds, 3)
        result["total_seconds"] = round(result["seconds"] + layer_restore_seconds + cargo_restore_seconds, 3)
        result["dependency_hit"] = result["local_cache_imports"] == 1 and result["cached_dependency_steps"] > 0
        name = "am-layer-reuse:" + secrets.token_hex(6)
        run(["skopeo", "copy", f"oci-archive:{image}", f"docker-daemon:{name}"])
        try:
            catalog = run(["docker", "run", "--rm", "--network", "none", "--entrypoint", "/usr/local/bin/cma", name, "--index-catalog"])
            if json.loads(catalog)["schema"] != json.loads(NEW):
                raise ValueError("warm image missed the edited module")
            run(["docker", "run", "--rm", "--network", "none", "--entrypoint", "/usr/local/bin/cma", name, "--allocator-self-test"])
            result["edited_catalog_and_allocator"] = "passed"
        finally:
            run(["docker", "image", "rm", name])


def diagnose(root, work, evidence):
    sys.path.insert(0, str(root / ".github/scripts"))
    from staging_cd_build import identity, snapshot
    from staging_cd_reservations import next_reservation
    revision = run(["git", "rev-parse", "HEAD"], cwd=root).decode().strip()
    if revision != CANDIDATE_SHA:
        raise ValueError("unexpected profile candidate source")
    tree = run(["git", "rev-parse", revision + "^{tree}"], cwd=root).decode().strip()
    reservation = next_reservation("am", revision, tree, 1, "patch", int(os.environ["GITHUB_RUN_ID"]),
                                   int(os.environ["GITHUB_RUN_ATTEMPT"]), [], [])
    source = work / "source"
    snapshot(root, revision, source)
    _, recipe, arguments = identity(reservation, source, staging_am_opt_level="0")
    layers = work / "layers"
    with builder(layers, seed=True) as (cache, namespace, name):
        args = ["docker", "buildx", "build", "--progress=rawjson", "--platform", "linux/amd64", "--builder", name,
                "--file", str(source / recipe["dockerfile"]), "--target", "dependencies", "--output", "type=cacheonly",
                "--cache-to", f"type=local,mode=min,image-manifest=true,oci-mediatypes=true,dest={layers}",
                "--build-arg", "BUILDKIT_CACHE_MOUNT_NS=" + namespace]
        for key, value in sorted(arguments.items()):
            args += ["--build-arg", key + "=" + value]
        cache.run([*args, str(source)], timeout=1800)
        evidence["dependency_reconstruction"] = cache.timing
    members = [p for p in layers.rglob("*") if p.is_file() and not p.is_symlink()]
    evidence["layer_bytes"] = sum(p.stat().st_size for p in members)
    evidence["layer_members"] = len(members)
    directory_shapes = []
    known = {"blobs", "sha256", "ingest", "ingest-locks", "tmp", "temp", "metadata", "refs", "snapshots"}
    locks = layers / "ingest-locks"
    if locks.is_dir() and not locks.is_symlink():
        lock_files = list(locks.iterdir())
        evidence["ingest_locks"] = {"count": len(lock_files), "empty_regular": sum(
            p.is_file() and not p.is_symlink() and p.stat().st_size == 0 for p in lock_files)}
    for path in sorted(layers.rglob("*")):
        if not path.is_dir() and not path.is_symlink():
            continue
        parts = path.relative_to(layers).parts
        if parts in (("blobs",), ("blobs", "sha256"), ("ingest",)) and not path.is_symlink():
            continue
        directory_shapes.append({"shape": [part if part in known else "opaque" for part in parts],
                                 "symlink": path.is_symlink(),
                                 "empty": path.is_dir() and not path.is_symlink() and not any(path.iterdir())})
    evidence["unexpected_directory_shapes"] = directory_shapes[:64]
    evidence["unexpected_directory_count"] = len(directory_shapes)
    # The validator is unmodified. Report only its fixed literal refusal,
    # never names, arbitrary exception messages, or OCI metadata payloads.
    classify = '''import ast, json, sys
from pathlib import Path
from buildkit_cache_archive import seal
known = set()
for path in ("buildkit_cache_archive.py", "actions_cache_buildkit_inventory.py"):
    for node in ast.walk(ast.parse(Path(path).read_text())):
        if isinstance(node, ast.Call) and isinstance(node.func, ast.Name):
            if node.func.id == "require" and len(node.args) == 2:
                value = node.args[1]
            elif node.func.id == "ValueError" and node.args:
                value = node.args[0]
            else:
                continue
            if isinstance(value, ast.Constant) and isinstance(value.value, str):
                known.add(value.value)
try:
    seal(Path(sys.argv[1]), Path(sys.argv[2]))
    result = {"status": "sealed"}
except Exception as error:
    result = {"status": "withheld", "type": type(error).__name__,
              "reason": str(error) if str(error) in known else "unclassified"}
print(json.dumps(result))
'''
    evidence["seal"] = json.loads(run([sys.executable, "-B", "-c", classify, str(layers), str(work / "layers.tar")],
                                     cwd=CONTROL / ".github/scripts", timeout=360))
    if evidence["seal"]["status"] != "sealed":
        return
    validator = CONTROL / ".github/scripts/buildkit_cache_archive.py"
    start = time.monotonic()
    run([sys.executable, "-B", str(validator), "restore", str(work / "restored"), str(work / "layers.tar")], timeout=360)
    evidence["restore_seconds"] = round(time.monotonic() - start, 3)
    edited = edit(root, work / "index")
    edited_tree = run(["git", "rev-parse", edited + "^{tree}"], cwd=root).decode().strip()
    updated = next_reservation("am", edited, edited_tree, 1, "patch", int(os.environ["GITHUB_RUN_ID"]),
                               int(os.environ["GITHUB_RUN_ATTEMPT"]), [], [reservation])
    warm = work / "edited"
    snapshot(root, edited, warm)
    _, recipe, arguments = identity(updated, warm, staging_am_opt_level="0")
    with builder(work / "restored", seed=False) as (cache, namespace, name):
        args = ["docker", "buildx", "build", "--progress=rawjson", "--platform", "linux/amd64", "--builder", name,
                "--file", str(warm / recipe["dockerfile"]), "--target", "dependencies", "--output", "type=cacheonly",
                "--cache-from", f"type=local,src={work / 'restored'}", "--build-arg", "BUILDKIT_CACHE_MOUNT_NS=" + namespace]
        for key, value in sorted(arguments.items()):
            args += ["--build-arg", key + "=" + value]
        cache.run([*args, str(warm)], timeout=1800)
        evidence["dependency_replay"] = cache.timing
        evidence["dependency_hit"] = cache.timing["local_cache_imports"] == 1 and cache.timing["cached_dependency_steps"] > 0


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--candidate", type=Path, required=True)
    parser.add_argument("--output", type=Path, required=True)
    parser.add_argument("--diagnose", action="store_true")
    args = parser.parse_args()
    evidence = {"source": CANDIDATE_SHA, "opt_level": "0", "status": "incomplete",
                "runner_cpu_count": os.cpu_count(),
                "runner_cpu_model": next((line.split(":", 1)[1].strip() for line in Path("/proc/cpuinfo").read_text().splitlines() if line.startswith("model name")), "unknown")}
    try:
        with tempfile.TemporaryDirectory(prefix="am-layer-reuse-", dir=os.environ["RUNNER_TEMP"]) as temporary:
            operation = diagnose if args.diagnose else benchmark
            operation(args.candidate.resolve(), Path(temporary), evidence)
        evidence["status"] = "measured"
    finally:
        args.output.write_text(json.dumps(evidence, indent=2, sort_keys=True) + "\n")


if __name__ == "__main__":
    main()
