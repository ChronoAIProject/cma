"""Component-scoped GHA layers and bounded Cargo mounts for fresh builders."""
import hashlib
import datetime
import importlib.util
import json
import os
import shutil
import subprocess
import tarfile
from pathlib import Path

from staging_cd_common import require
from staging_cd_cache_context import context

SCOPES = {"am": "cma-am", "eg": "cma-eg", "web": "cma-web",
          "runtime": "cma-runtime", "runtime-omarchy": "cma-runtime-omarchy"}
# CI's runtime index and the CD runtime index share content-addressed build
# stages with Omarchy. Keep each publisher's export in its own scope.
EXTRA_IMPORTS = {"runtime": ("runtime",), "runtime-omarchy": ("cma-runtime", "runtime")}
RUNTIME_KEYS = ("ACTIONS_RUNTIME_TOKEN", "ACTIONS_RESULTS_URL")


def bridge():
    spec = importlib.util.spec_from_file_location("cargo_cache_bridge", Path(__file__).with_name("api-cargo-cache.py"))
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


def layer_options(env, component):
    require(env.get("GITHUB_REPOSITORY") == "ChronoAIProject/cma" and component in SCOPES,
            "Build cache repository or component is invalid")
    # GitHub authorizes cache entries by ref. A PR can restore the protected
    # branch's entries but cannot write that ref's namespace. Never use a
    # registry exporter or copy a PR artifact into the development namespace.
    def cache(scope):
        return "type=gha,version=2,scope=" + scope + ",timeout=2m"
    args = []
    for scope in (SCOPES[component], *EXTRA_IMPORTS.get(component, ())):
        args.extend(("--cache-from", cache(scope)))
    if env.get("GITHUB_EVENT_NAME") == "push" and env.get("GITHUB_REF") == "refs/heads/develop":
        args += ["--cache-to", cache(SCOPES[component]) + ",mode=max,ignore-error=true"]
    return args


def identity(root, component):
    manifest = json.loads((root / ".github/scripts/deployables.json").read_text())
    from staging_cd_versions import DEPLOYABLES
    recipe = manifest[DEPLOYABLES[component]]
    paths = [root / recipe["dockerfile"], root / ".dockerignore", root / "api/Cargo.lock",
             root / "api/rust-toolchain.toml", root / "api/Cargo.toml"]
    paths += sorted((root / "api/crates").glob("*/Cargo.toml"))
    paths += sorted((root / "api/.cargo").rglob("*"))
    source_inputs = {str(p.relative_to(root)): hashlib.sha256(p.read_bytes()).hexdigest()
                     for p in paths if p.is_file()}
    tools = [Path(__file__), Path(__file__).with_name("api-cargo-cache.py")]
    payload = {"format": 2, "component": component, "platform": "linux/amd64",
               "profile": "release", "recipe": recipe,
               "refresh_day": datetime.datetime.now(datetime.timezone.utc).date().isoformat(),
               "inputs": source_inputs,
               "tools": {p.name: hashlib.sha256(p.read_bytes()).hexdigest() for p in tools}}
    return hashlib.sha256(json.dumps(payload, sort_keys=True).encode()).hexdigest()


class BuildCache:
    def __init__(self, root, env, source, component):
        context(env)  # Authority failures are never downgraded to cache misses.
        require(component == env["CD_COMPONENT"], "Build cache component differs")
        self.component, self.env = component, env
        self.root = Path(env["RUNNER_TEMP"]) / "cma-build-cache"
        require(not self.root.is_symlink(), "Build cache root is invalid")
        self.root.mkdir(mode=0o700, exist_ok=True)
        self.accepted, self.pending = self.root / "accepted", self.root / "pending"
        self.compatibility = identity(source, component)
        self.restored = False

    def restore(self):
        if self.component not in ("am", "eg") or not self.accepted.exists():
            return False
        try:
            require(self.accepted.is_dir() and not self.accepted.is_symlink(),
                    "Build cache accepted root is invalid")
            tool = bridge()
            tool.validate_bundle(self.accepted, self.compatibility)
            tool.transfer("inject", self.accepted)
            self.restored = True
        except (OSError, ValueError, KeyError, TypeError, EOFError, tarfile.TarError, subprocess.SubprocessError):
            self.restored = False
        return self.restored

    def build_options(self):
        return layer_options(self.env, self.component) if self.build_environment() else []

    def build_environment(self):
        # Buildx's cache client needs these; build steps, registry transport and
        # scanners never receive them as build arguments, secrets or mounts.
        if not all(self.env.get(key) for key in RUNTIME_KEYS):
            return {}
        return {key: self.env[key] for key in RUNTIME_KEYS}

    def export_mounts(self):
        if self.component not in ("am", "eg") or self.restored:
            return False
        require(not self.pending.exists(), "Cargo cache export must be fresh")
        try:
            tool = bridge()
            tool.transfer("extract", self.pending)
            tool.seal(self.pending, self.compatibility)
            if self.accepted.exists():
                require(not self.accepted.is_symlink(), "Build cache accepted root is invalid")
                shutil.rmtree(self.accepted)
            self.pending.rename(self.accepted)
            return True
        except (OSError, ValueError, KeyError, TypeError, EOFError, tarfile.TarError, subprocess.SubprocessError):
            return False
        finally:
            if self.pending.exists() and not self.pending.is_symlink():
                shutil.rmtree(self.pending)


def prepare(root, env):
    context(env)
    component = env["CD_COMPONENT"]
    require(component in ("am", "eg"), "Cargo caching is limited to Rust services")
    prefix = f"cma-cd-cargo-v2-{component}-{identity(root, component)}-"
    return {"directory": str(Path(env["RUNNER_TEMP"]) / "cma-build-cache/accepted"),
            "prefix": prefix, "key": prefix + env["GITHUB_RUN_ID"] + "-" + env["GITHUB_RUN_ATTEMPT"]}


if __name__ == "__main__":
    with Path(os.environ["GITHUB_OUTPUT"]).open("a") as output:
        for key, value in prepare(Path.cwd(), os.environ).items():
            output.write(f"{key}={value}\n")
