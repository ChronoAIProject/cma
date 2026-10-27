#!/usr/bin/env python3
"""Manual proof with two disposable builders; intentionally not on every CI run."""
import importlib.util
import os
from pathlib import Path
import re
import subprocess
import tempfile
import time
import uuid


SPEC = importlib.util.spec_from_file_location("api_cargo_cache", Path(__file__).with_name("api-cargo-cache.py"))
cache = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(cache)


def command(*arguments, **kwargs):
    return subprocess.run(arguments, text=True, check=True, timeout=180, **kwargs)


def sources(directory):
    files = {
        "Cargo.toml": '[workspace]\nresolver = "2"\nmembers = ["cache_app", "cache_dep"]\n',
        "Cargo.lock": 'version = 4\n\n[[package]]\nname = "cache_app"\nversion = "0.1.0"\ndependencies = ["cache_dep"]\n\n[[package]]\nname = "cache_dep"\nversion = "0.1.0"\n',
        "cache_app/Cargo.toml": '[package]\nname = "cache_app"\nversion = "0.1.0"\nedition = "2021"\n[dependencies]\ncache_dep = { path = "../cache_dep" }\n',
        "cache_dep/Cargo.toml": '[package]\nname = "cache_dep"\nversion = "0.1.0"\nedition = "2021"\n',
        "cache_dep/src/lib.rs": 'pub fn answer() -> u32 { 42 }\n',
        "cache_app/src/main.rs": 'fn main() { println!("{}", cache_dep::answer()); }\n',
    }
    for name, source in files.items():
        path = directory / name
        path.parent.mkdir(parents=True, exist_ok=True)
        path.write_text(source)
        # Archive second-level timestamps must still be newer than old sources.
        os.utime(path, (time.time() - 120, time.time() - 120))
    definition = (cache.ROOT / "infra/api/Dockerfile").read_text()
    rust = re.search(r"^FROM (rust:[^\s]+) AS build$", definition, re.MULTILINE).group(1)
    (directory / "Dockerfile").write_text(f"""FROM {rust} AS build
RUN apk add --no-cache musl-dev
WORKDIR /build
COPY . .
ARG EXPECT_WARM=false
RUN --mount=type=cache,id=cma-api-registry,target=/usr/local/cargo/registry,sharing=locked \\
    --mount=type=cache,id=cma-api-target-amd64,target=/build/target,sharing=locked \\
    set -eu; if [ "$EXPECT_WARM" = true ]; then test -f /usr/local/cargo/registry/cache-proof; \\
    else touch /usr/local/cargo/registry/cache-proof; fi; \\
    cargo build --release --locked --offline -vv --bin cache_app; \\
    mkdir /out; /build/target/release/cache_app > /out/result
FROM scratch
COPY --from=build /out/ /
""")


def build(builder, source, destination, warm):
    result = command("docker", "buildx", "build", "--builder", builder, "--platform", "linux/amd64",
                     "--progress=plain", "--build-arg", f"EXPECT_WARM={str(warm).lower()}",
                     "--output", f"type=local,dest={destination}", str(source), capture_output=True)
    return result.stdout + result.stderr


def main():
    # Do not bootstrap or repair the user's Docker daemon as a side effect.
    subprocess.run(["docker", "info"], check=True, timeout=10, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
    active = set()
    try:
        with tempfile.TemporaryDirectory(prefix="cma-api-cache-proof-") as temporary:
            root = Path(temporary)
            source = root / "source"
            sources(source)
            first, second = ["cma-cache-proof-" + uuid.uuid4().hex[:12] for _ in range(2)]
            for builder in (first,):
                command("docker", "buildx", "create", "--name", builder, "--driver", "docker-container")
                active.add(builder)
            cold = build(first, source, root / "cold", False)
            cache.require("Compiling cache_dep" in cold, "cold build did not compile the dependency")
            cache.require((root / "cold/result").read_text().strip() == "42", "cold executable differs")
            cache.transfer("extract", root / "bundle", first)
            cache.seal(root / "bundle", "manual-fresh-builder-proof-")
            command("docker", "buildx", "rm", first)
            active.remove(first)
            command("docker", "buildx", "create", "--name", second, "--driver", "docker-container")
            active.add(second)
            cache.validate_bundle(root / "bundle", "manual-fresh-builder-proof-")
            cache.transfer("inject", root / "bundle", second)
            (source / "cache_app/src/main.rs").write_text('fn main() { println!("{}", cache_dep::answer() + 1); }\n')
            warm = build(second, source, root / "warm", True)
            cache.require("Fresh cache_dep" in warm, "dependency was not reused in the fresh builder")
            cache.require("Compiling cache_app" in warm, "changed application source was not recompiled")
            cache.require((root / "warm/result").read_text().strip() == "43", "warm cache hid changed executable content")
            print("PASS: fresh builder restored both mounts, reused the dependency and rebuilt changed executable content (42 -> 43).")
    finally:
        for builder in active:
            command("docker", "buildx", "rm", builder)


if __name__ == "__main__":
    main()
