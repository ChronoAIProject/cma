"""Component-scoped GHA layers and bounded Cargo mounts for fresh builders."""
import importlib.util
import json
import os
import shutil
import subprocess
import tarfile
import tempfile
import uuid
from pathlib import Path

from staging_cd_build import snapshot
from staging_cd_build_cargo_keys import identity
from staging_cd_common import require
from staging_cd_cache_context import context
from staging_cd_reservations import validate_reservation

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


class BuildCache:
    def __init__(self, root, env, source, component):
        context(env)  # Authority failures are never downgraded to cache misses.
        require(component == env["CD_COMPONENT"], "Build cache component differs")
        self.component, self.env = component, env
        self.root = Path(env["RUNNER_TEMP"]) / "cma-build-cache"
        require(not self.root.is_symlink(), "Build cache root is invalid")
        self.root.mkdir(mode=0o700, exist_ok=True)
        self.accepted, self.pending = self.root / "accepted", self.root / "pending"
        self.key = identity(source, component) if component in ("am", "eg") else None
        self.namespace = uuid.uuid4().hex
        self.restore_result = "miss"

    def restore(self):
        self.restore_result = "miss"
        self.namespace = uuid.uuid4().hex
        if self.key is None or not self.accepted.exists():
            return False
        try:
            require(self.accepted.is_dir() and not self.accepted.is_symlink(),
                    "Build cache accepted root is invalid")
            tool = bridge()
            manifest = tool.validate_bundle(self.accepted, self.key.prefix, family=self.key.family)
            exact = manifest["prefix"] == self.key.prefix
            # A different graph must let Cargo resolve and verify its own registry.
            tool.transfer("inject" if exact else "inject-target", self.accepted, namespace=self.namespace)
            self.restore_result = "hit" if exact else "partial"
        except (OSError, ValueError, KeyError, TypeError, EOFError, tarfile.TarError, subprocess.SubprocessError):
            # Extraction can fail after writing some files. Abandon that namespace.
            self.namespace = uuid.uuid4().hex
            return False
        return True

    def build_options(self):
        args = layer_options(self.env, self.component) if self.build_environment() else []
        if self.key is not None:
            args += ["--build-arg", "BUILDKIT_CACHE_MOUNT_NS=" + self.namespace]
        return args

    def build_environment(self):
        # Buildx's cache client needs these; build steps, registry transport and
        # scanners never receive them as build arguments, secrets or mounts.
        if not all(self.env.get(key) for key in RUNTIME_KEYS):
            return {}
        return {key: self.env[key] for key in RUNTIME_KEYS}

    def export_mounts(self):
        # Refresh after exact hits too. Restoring the first bundle of the day all
        # day long made each build recompile everything merged since that seed.
        if self.key is None:
            return False
        require(not self.pending.exists(), "Cargo cache export must be fresh")
        try:
            tool = bridge()
            tool.transfer("extract", self.pending, namespace=self.namespace)
            tool.seal(self.pending, self.key.prefix, family=self.key.family)
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


def lookup_key(root, component, env):
    # The release controller can publish a reserved source older than its checkout.
    # One Actions restore seeds the first build. Each later build validates its own family.
    tasks = json.loads(env["CD_TASKS"])
    require(isinstance(tasks, list) and tasks, "Cargo cache publication tasks are missing")
    for task in tasks:
        require(isinstance(task, dict) and task.get("component") == component
                and task.get("mode") in ("publish", "resume"), "Cargo cache task differs")
        if task["mode"] == "publish":
            reservation = validate_reservation(task.get("reservation"))
            require(reservation["component"] == component, "Cargo cache reservation differs")
            if reservation["source"] != env["GITHUB_SHA"]:
                with tempfile.TemporaryDirectory(dir=env["RUNNER_TEMP"]) as directory:
                    source = Path(directory) / "source"
                    snapshot(root, reservation["source"], source)
                    return identity(source, component)
            break
    return identity(root, component)


def prepare(root, env):
    context(env)
    component = env["CD_COMPONENT"]
    require(component in ("am", "eg"), "Cargo caching is limited to Rust services")
    key = lookup_key(root, component, env)
    return {"directory": str(Path(env["RUNNER_TEMP"]) / "cma-build-cache/accepted"),
            "prefix": key.prefix, "family": key.family, "key": key.archive_key(env)}


if __name__ == "__main__":
    with Path(os.environ["GITHUB_OUTPUT"]).open("a") as output:
        for key, value in prepare(Path.cwd(), os.environ).items():
            output.write(f"{key}={value}\n")
