#!/usr/bin/env python3
"""Time a protected-format seed and a fresh builder using the publication recipe."""

import argparse
import base64
import json
import os
from pathlib import Path
import re
import secrets
import subprocess
import tempfile
import time

from staging_cd_build import build, inspect
from staging_cd_build_cache import bridge, source_identity
from staging_cd_build_cargo_keys import CargoKey
from staging_cd_build_progress import summarize
from staging_cd_reservations import next_reservation

ROOT = Path(__file__).resolve().parents[2]
BINARIES = {"am": "cma", "eg": "cmaeg"}
WARM_MAX_SECONDS = 540
LIBRARY_PROBE = "workspace-library-probe"
LIBRARY_EDITS = {
    "am": ("store/migrations/index_toolkit/mod.rs", b'"index-toolkit-catalog-v1"',
           json.dumps(LIBRARY_PROBE).encode(), "--index-catalog", ("schema",)),
    "eg": ("openapi.rs", b"sort(&mut document);",
           f'document["info"]["title"] = serde_json::json!("{LIBRARY_PROBE}");\n    sort(&mut document);'.encode(),
           "--print-openapi", ("info", "title")),
}


def run(args, *, input=None, env=None, timeout=180):
    return subprocess.run(args, cwd=ROOT, input=input, env=env, timeout=timeout, check=True,
                          capture_output=True)


def git(*args, input=None, env=None):
    result = run(["git", *args], input=input, env=env)
    return result.stdout.decode().strip()


def edited_revision(seed, component, index, *, library=False):
    file = "main.rs"
    marker = b"fn main() {"
    replacement = marker + b'''
    if std::env::args_os().nth(1).is_some_and(|arg| arg == "--workspace-cache-probe") {
        println!("workspace-probe:{}:{}", env!("CHRONO_BUILD_SHA"), env!("CHRONO_BUILD"));
        return;
    }
'''
    if library:
        file, marker, replacement, _, _ = LIBRARY_EDITS[component]
    path = f"api/crates/{BINARIES[component]}/src/{file}"
    source = run(["git", "show", f"{seed}:{path}"]).stdout
    if source.count(marker) != 1:
        raise ValueError(f"Cannot add a single benchmark probe to {path}")
    content = source.replace(marker, replacement, 1)
    env = dict(os.environ, GIT_INDEX_FILE=str(index), GIT_AUTHOR_NAME="Image Benchmark",
               GIT_AUTHOR_EMAIL="benchmark@example.invalid", GIT_COMMITTER_NAME="Image Benchmark",
               GIT_COMMITTER_EMAIL="benchmark@example.invalid")
    git("read-tree", seed, env=env)
    blob = git("hash-object", "-w", "--stdin", input=content)
    git("update-index", "--cacheinfo", f"100644,{blob},{path}", env=env)
    tree = git("write-tree", env=env)
    revision = git("commit-tree", tree, "-p", seed, "-m", "Exercise one service source edit", env=env)
    if git("diff-tree", "--no-commit-id", "--name-only", "-r", seed, revision) != path:
        raise ValueError("Benchmark revision must change exactly one source file")
    return revision


class Builder:
    def __init__(self, layers, cold):
        self.name = "cma-workspace-proof-" + secrets.token_hex(6)
        self.namespace = secrets.token_hex(12)
        self.layers, self.cold = layers, cold
        self.report = None

    def __enter__(self):
        run(["docker", "buildx", "create", "--name", self.name, "--driver", "docker-container"])
        return self

    def __exit__(self, *_):
        run(["docker", "buildx", "rm", self.name])

    def build_options(self):
        layer_cache = f"type=local,mode=max,dest={self.layers}" if self.cold else f"type=local,src={self.layers}"
        return ["--builder", self.name, "--build-arg", "BUILDKIT_CACHE_MOUNT_NS=" + self.namespace,
                "--cache-to" if self.cold else "--cache-from", layer_cache]

    def build_environment(self):
        return {}

    def command(self, args, *, timeout, **_):
        if args[:3] == ["docker", "buildx", "build"]:
            started = time.monotonic()
            result = run(args, timeout=min(timeout, 5400))
            progress = result.stderr.decode(errors="replace")
            self.report = {"seconds": round(time.monotonic() - started, 3),
                           **summarize(progress), **workspace_units(progress)}
            if not self.report["complete"]:
                raise ValueError("BuildKit timing receipt incomplete")
            return result.stdout
        return run(args, timeout=timeout).stdout


def workspace_units(progress):
    # Preserve observations of Rust work units while omitting raw BuildKit
    # lines, which can contain command-line or build environment details.
    by_vertex = {}
    for line in progress.splitlines():
        try:
            event = json.loads(line)
        except ValueError:
            continue
        if not isinstance(event, dict):
            continue
        for entry in event.get("logs", []):
            if "data" in entry:
                data = base64.b64decode(entry["data"]).decode(errors="replace")
                by_vertex.setdefault(entry["vertex"], []).append(data)
    # Chef can compile a placeholder with the same target name. Only the
    # publication build that ran the workspace guard proves actual reuse.
    vertices = ["".join(chunks) for chunks in by_vertex.values()]
    messages = "\n".join(vertex for vertex in vertices if "Workspace cache " in vertex)
    fresh = sorted(set(re.findall(r"(?m)^\s*Fresh ([A-Za-z0-9_-]+) v", messages)))
    rebuilt = sorted(set(re.findall(r"(?m)^\s*Compiling ([A-Za-z0-9_-]+) v", messages)))
    state = re.findall(r"(?m)^Workspace cache compatible\. Invalidated: ([A-Za-z0-9_, -]+)$", messages)
    retained = re.findall(r"(?m)^Workspace cache retained (\S+) library under its release audit$", messages)
    targets = re.findall(r"(?m)^\s*Running [^\n]*[/ ]crates/(cma|cmaeg)/src/(lib|main)\.rs\s", messages)
    return {"fresh": fresh, "compiled": rebuilt, "workspace_invalidation": state,
            "retained_workspace_libraries": retained, "service_targets": sorted({":".join(pair) for pair in targets})}


def reservation(component, revision, pending=()):
    return next_reservation(
        component, revision, git("rev-parse", revision + "^{tree}"), 1, "patch",
        int(os.environ["GITHUB_RUN_ID"]), int(os.environ["GITHUB_RUN_ATTEMPT"]), [], pending,
    )


def measure(builder, reserved, directory):
    directory.mkdir()
    archive, release = build(ROOT, reserved, directory, runner=builder.command, cache=builder)
    digest = inspect(archive, release)
    return archive, {"archive_digest": digest, "revision": release["source"], "version": release["version"],
                     **builder.report}


def seal_seed(tool, bundle, builder, reserved, source, receipt):
    key = source_identity(source, reserved["component"], reserved["source"])
    # Exercise the protected format using a disposable archive. This benchmark
    # has read-only GitHub permissions and never writes an Actions cache.
    key = CargoKey(key.family.replace("cma-cd-cargo-v3-", "cma-cd-cargo-v4-", 1), key.graph, key.source)
    started = time.monotonic()
    tool.transfer("extract", bundle, builder.name, namespace=builder.namespace)
    receipt.update(export_seconds=round(time.monotonic() - started, 3),
                   compressed_bytes=sum(item.stat().st_size for item in bundle.glob("*.tar.gz")),
                   expanded_limit_exceeded=(bundle / "oversize").exists())
    tool.seal(bundle, key.prefix, family=key.family, source=key.source, graph=key.graph)
    receipt["bytes"] = sum(item.stat().st_size for item in bundle.iterdir())
    return key


def verify_image(archive, component, revision, version, *, library=False):
    name = "cma-workspace-probe:" + secrets.token_hex(8)
    run(["skopeo", "copy", f"oci-archive:{archive}", f"docker-daemon:{name}"], timeout=180)
    try:
        argument = LIBRARY_EDITS[component][3] if library else "--workspace-cache-probe"
        command = ["docker", "run", "--rm", "--network", "none", "--entrypoint",
                   f"/usr/local/bin/{BINARIES[component]}", name, argument]
        output = run(command, timeout=30).stdout.decode().strip()
        if library:
            value = json.loads(output)
            for field in LIBRARY_EDITS[component][4]:
                value = value[field]
            if value != LIBRARY_PROBE:
                raise ValueError("The warm executable did not contain the library edit")
        elif output != f"workspace-probe:{revision}:{version}":
            raise ValueError("The warm executable did not contain the one-file edit and current release identity")
    finally:
        run(["docker", "image", "rm", name])


def measure_warm(tool, key, work, component, reserved, case, result):
    with Builder(work / "layers", False) as builder:
        started = time.monotonic()
        tool.validate_bundle(work / "seed", key.prefix, family=key.family)
        tool.transfer("inject", work / "seed", builder.name, namespace=builder.namespace)
        restore = round(time.monotonic() - started, 3)
        archive, result[case] = measure(builder, reserved, work / case)
        result[case]["restore_seconds"] = restore
        verify_image(archive, component, reserved["source"], reserved["version"], library=case == "warm_library")
        archive.unlink()


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("component", choices=sorted(BINARIES))
    parser.add_argument("output", type=Path)
    args = parser.parse_args()
    seed = git("rev-parse", "HEAD")
    cold_request = reservation(args.component, seed)
    tool = bridge()
    result = {"component": args.component, "source": seed, "runner_cpu_count": os.cpu_count()}
    try:
        with tempfile.TemporaryDirectory(prefix="workspace-bench-") as temporary:
            work = Path(temporary)
            with Builder(work / "layers", True) as builder:
                archive, result["cold"] = measure(builder, cold_request, work / "cold")
                archive.unlink()
                result["seed"] = {}
                key = seal_seed(tool, work / "seed", builder, cold_request, work / "cold/source", result["seed"])
            warm = edited_revision(seed, args.component, work / "index")
            warm_request = reservation(args.component, warm, [cold_request])
            measure_warm(tool, key, work, args.component, warm_request, "warm", result)
            library = edited_revision(seed, args.component, work / "index", library=True)
            library_request = reservation(args.component, library, [cold_request])
            measure_warm(tool, key, work, args.component, library_request, "warm_library", result)
            binary = BINARIES[args.component]
            measured = result["warm"]
            if (measured["retained_workspace_libraries"] != [binary]
                    or measured["service_targets"] != [f"{binary}:main"]):
                raise ValueError("Executable-only build did not reuse its audited workspace library")
            if result["warm_library"]["service_targets"] != [f"{binary}:lib", f"{binary}:main"]:
                raise ValueError("Library edit did not compile the library and executable")
            if measured["seconds"] > WARM_MAX_SECONDS:
                raise ValueError("Warm image build exceeds 540 seconds")
            result["status"] = "pass"
    finally:
        args.output.write_text(json.dumps(result, indent=2, sort_keys=True) + "\n")
        print(json.dumps(result, sort_keys=True))


if __name__ == "__main__":
    main()
