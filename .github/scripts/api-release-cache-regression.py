#!/usr/bin/env python3
"""Exercise release recipes with changed archived source and a warm Cargo target.

Requires Python 3.12+ and Docker/Buildx. Fixtures are synthetic; this checks cache correctness,
not the real services' allocator or media qualification. Cache IDs are isolated
from normal builds. No image is published and no credentials are passed in.
"""
import argparse
import base64
import datetime
import importlib.util
import json
import os
import shutil
from pathlib import Path
import subprocess
import tempfile
import time
import uuid


ROOT = Path(__file__).resolve().parents[2]


def run(args, **kwargs):
    return subprocess.run(args, check=True, capture_output=True, text=True, **kwargs)


def fixture(root, binary):
    files = {
        "Cargo.toml": f'[workspace]\nresolver="2"\nmembers=["crates/{binary}","library"]\n',
        f"crates/{binary}/Cargo.toml": f'[package]\nname="{binary}"\nversion="0.1.0"\nedition="2021"\n'
                          '[dependencies]\ncache_library={path="../../library"}\n',
        f"crates/{binary}/src/main.rs": '''fn main() {
    if std::env::args().any(|a| a == "--media-sandbox-check") {
        std::process::exit(if cfg!(target_arch = "aarch64") { 125 } else { 0 });
    }
    println!("{}:{}:{}", cache_library::value(), cache_library::generated(),
             env!("CHRONO_BUILD_SHA"));
}
''',
        "library/Cargo.toml": '[package]\nname="cache_library"\nversion="0.1.0"\nedition="2021"\n'
                              '[dependencies]\nmemchr="=2.7.6"\n',
        "library/src/lib.rs": '''pub fn value() -> &'static str { "first" }
pub fn generated() -> &'static str { include!(concat!(env!("OUT_DIR"), "/value.rs")) }
''',
        "library/build.rs": '''fn main() {
    println!("cargo:rerun-if-changed=value.txt");
    let value = std::fs::read_to_string("value.txt").unwrap();
    let path = std::path::PathBuf::from(std::env::var("OUT_DIR").unwrap()).join("value.rs");
    std::fs::write(path, format!("{:?}", value.trim())).unwrap();
}
''',
        "library/value.txt": "generated-first\n",
    }
    for relative, content in files.items():
        path = root / "api" / relative
        path.parent.mkdir(parents=True, exist_ok=True)
        path.write_text(content)
        os.utime(path, (1, 1))


def current_logs(raw, started):
    lines = []
    for line in raw.splitlines():
        try:
            event = json.loads(line)
        except ValueError:
            continue
        for row in event.get("logs", []):
            stamp = datetime.datetime.fromisoformat(row["timestamp"].replace("Z", "+00:00"))
            # BuildKit may replay historical logs for reused vertices.
            if stamp.timestamp() >= started:
                lines.append(base64.b64decode(row["data"]).decode(errors="replace"))
    return "".join(lines)


def transfer_cache(source, root, prefix, platform):
    """Use the bounded production archive format with isolated fixture mounts."""
    spec = importlib.util.spec_from_file_location(
        "release_cache_bridge", source / ".github/scripts/api-cargo-cache.py")
    bridge = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(bridge)
    architecture = platform.split("/")[-1]
    bridge.MOUNTS = {"registry": prefix + "-registry", "target": prefix + "-target-" + architecture}
    bundle = root / "bundle"
    command = ["docker", "buildx", "build", "--platform", platform,
               "--build-arg", "CMA_CACHE_TRANSFER=" + uuid.uuid4().hex, "--file", "-"]
    run(command + ["--output", "type=local,dest=" + str(bundle), str(root)],
        input=bridge.dockerfile("extract"))
    bridge.seal(bundle, prefix)
    bridge.validate_bundle(bundle, prefix)
    bridge.MOUNTS["target"] = prefix + "-restored-target-" + architecture
    run(command + ["--output", "type=cacheonly", str(bundle)],
        input=bridge.dockerfile("inject"))


def exercise(source, binary, platform):
    recipe = (source / f"api/crates/{binary}/Dockerfile").read_text()
    cache_prefix = "release-regression-" + uuid.uuid4().hex
    recipe = recipe.replace("id=cma-api-", "id=" + cache_prefix + "-")
    recipe += f"\nFROM scratch AS cache-regression-output\nCOPY --from=build /out/{binary} /program\n"
    # Use the same official native toolchain as the release recipe.
    toolchain = "rust:1.96.1-alpine"
    if f"FROM {toolchain} AS build" not in recipe:
        raise ValueError("review the regression toolchain when the release builder changes")
    with tempfile.TemporaryDirectory(prefix="cma-release-regression-") as directory:
        root = Path(directory)
        fixture(root, binary)
        for name in ("workspace_cache.py", "release_rustc.py"):
            helper = Path("api/build_support") / name
            if (source / helper).is_file():
                (root / helper).parent.mkdir(parents=True, exist_ok=True)
                shutil.copy2(source / helper, root / helper)
        (root / "api/rust-toolchain.toml").write_text((source / "api/rust-toolchain.toml").read_text())
        (root / "Dockerfile").write_text(recipe)
        run(["docker", "run", "--rm", "--platform", platform, "--mount",
             f"type=bind,source={root / 'api'},target=/build", "--workdir", "/build",
             toolchain, "cargo", "generate-lockfile"])
        results = []
        for version in ("first", "second", "restored"):
            if version != "first":
                path = root / "api/library/src/lib.rs"
                previous = "first" if version == "second" else "second"
                path.write_text(path.read_text().replace(f'"{previous}"', f'"{version}"'))
                (root / "api/library/value.txt").write_text(f"generated-{version}\n")
                # Changed, newer source can still predate restored build outputs.
                for path in (path, root / "api/library/value.txt"):
                    os.utime(path, (2, 2))
            if version == "restored":
                transfer_cache(source, root, cache_prefix, platform)
                (root / "Dockerfile").write_text(recipe.replace(
                    "id=" + cache_prefix + "-target-", "id=" + cache_prefix + "-restored-target-"))
            output = root / (version + "-output")
            started = time.time()
            built = run(["docker", "buildx", "build", "--platform", platform,
                         "--progress=rawjson", "--target", "cache-regression-output",
                         "--build-arg", "CHRONO_BUILD=" + cache_prefix,
                         "--build-arg", "CHRONO_BUILD_SHA=build-" + version,
                         "--output", "type=local,dest=" + str(output), str(root)])
            elapsed = time.time() - started
            actual = run(["docker", "run", "--rm", "--platform", platform, "--mount",
                          f"type=bind,source={output},target=/out,readonly", toolchain,
                          "/out/program"]).stdout.strip()
            expected = f"{version}:generated-{version}:build-{version}"
            if actual != expected:
                raise AssertionError(f"{binary}: stale cached behavior: {actual!r}; expected {expected!r}")
            if version != "first" and "Compiling memchr " in current_logs(built.stderr, started):
                raise AssertionError("external dependency was unnecessarily rebuilt")
            results.append({"case": version, "seconds": round(elapsed, 3), "behavior": actual})
        return {"component": binary, "platform": platform, "passed": True, "builds": results}


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--source-root", type=Path, default=ROOT)
    parser.add_argument("--component", choices=("cma", "cmaeg", "both"), default="both")
    parser.add_argument("--platform", choices=("linux/amd64", "linux/arm64"), default="linux/amd64")
    args = parser.parse_args()
    for binary in (("cma", "cmaeg") if args.component == "both" else (args.component,)):
        print(json.dumps(exercise(args.source_root, binary, args.platform)), flush=True)


if __name__ == "__main__":
    main()
