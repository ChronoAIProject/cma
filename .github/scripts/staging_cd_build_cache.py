"""Component-scoped GHA layers and bounded Cargo mounts for fresh builders."""
import importlib.util
import json
import os
import shutil
import subprocess
import tarfile
import tempfile
import time
import uuid
from contextlib import contextmanager, suppress
from datetime import datetime, timezone
from pathlib import Path

from staging_cd_build import snapshot
from staging_cd_build_cargo_keys import identity
from staging_cd_common import require
from staging_cd_cache_context import context
from staging_cd_cargo_artifact import MAX_BUNDLE_BYTES, MAX_EXPANDED_BYTES
from staging_cd_process import terminate
from staging_cd_reservations import validate_reservation

SCOPES = {"am": "cma-cd-am", "eg": "cma-cd-eg", "web": "cma-cd-web",
          "runtime": "cma-runtime", "runtime-omarchy": "cma-runtime-omarchy"}
# The CI image builds export cma-am, cma-eg, cma-web, runtime and
# runtime-omarchy (ci-images.yml, ci-omarchy.yml). Keep each publisher's
# export in its own scope and only read the other's. CD builds from a
# git-archive snapshot whose file modes differ from CI's checkout, so a shared
# scope made each writer replace the other's index and cost the next build its
# dependency stages (cargo-chef install and cook, 5-10 min for AM).
CI_SCOPES = frozenset({"cma-am", "cma-eg", "cma-web", "runtime", "runtime-omarchy"})
EXTRA_IMPORTS = {"am": ("cma-am",), "eg": ("cma-eg",), "web": ("cma-web",),
                 "runtime": ("runtime",), "runtime-omarchy": ("cma-runtime", "runtime")}
RUNTIME_KEYS = ("ACTIONS_RUNTIME_TOKEN", "ACTIONS_RESULTS_URL")
EXPORT_ERRORS = (OSError, ValueError, KeyError, TypeError, EOFError, tarfile.TarError, subprocess.SubprocessError)


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
    # The GHA exporter cannot cap or reserve its bytes before uploading. Keep
    # the existing scopes readable throughout the migration to sealed archives.
    return args


class BuildCache:
    def __init__(self, root, env, source, component, *, cache_day=None):
        context(env)  # Authority failures are never downgraded to cache misses.
        require(component == env["CD_COMPONENT"], "Build cache component differs")
        self.component, self.env = component, env
        self.root = Path(env["RUNNER_TEMP"]) / "cma-build-cache"
        require(not self.root.is_symlink(), "Build cache root is invalid")
        self.root.mkdir(mode=0o700, exist_ok=True)
        self.accepted, self.pending = self.root / "accepted", self.root / "pending"
        self.cache_day = cache_day if cache_day is not None else datetime.now(timezone.utc).date()
        self.key = identity(source, component, day=self.cache_day) if component in ("am", "eg") else None
        self.namespace = uuid.uuid4().hex
        self.restore_result = "miss"
        self.export_reason = None
        self.export_headroom = None
        self.export_retained_groups = None
        self.layer_root = Path(env["RUNNER_TEMP"]) / "cma-layer-cache"
        self.layer_export_requested = False

    def restore(self):
        self.restore_result = "miss"
        self.namespace = uuid.uuid4().hex
        if self.key is None or not self.accepted.exists():
            return False
        try:
            require(self.accepted.is_dir() and not self.accepted.is_symlink(),
                    "Build cache accepted root is invalid")
            tool = bridge()
            manifest = tool.validate_bundle(self.accepted, self.key.prefix, family=self.key.family,
                                            max_archive=MAX_BUNDLE_BYTES, max_content=MAX_EXPANDED_BYTES)
            exact = manifest["prefix"] == self.key.prefix
            # A different graph must let Cargo resolve and verify its own registry.
            tool.transfer("inject" if exact else "inject-target", self.accepted, namespace=self.namespace,
                          timeout=TRANSFER_SECONDS, max_content=MAX_EXPANDED_BYTES)
            self.restore_result = "hit" if exact else "partial"
        except (OSError, ValueError, KeyError, TypeError, EOFError, tarfile.TarError, subprocess.SubprocessError):
            # Extraction can fail after writing some files. Abandon that namespace.
            self.namespace = uuid.uuid4().hex
            return False
        return True

    def build_options(self):
        self.layer_export_requested = False
        args = layer_options(self.env, self.component) if self.build_environment() else []
        if self.env.get("CD_LAYER_ARCHIVE_CACHE") == "enabled":
            restored = self.layer_root / "restored"
            pending = self.layer_root / "pending"
            require(not self.layer_root.is_symlink() and not pending.is_symlink(), "Layer cache path is invalid")
            if (self.env.get("CD_LAYER_ARCHIVE_RESTORED") == "true"
                    and restored.is_dir() and not restored.is_symlink()):
                args += ["--cache-from", "type=local,src=" + str(restored)]
            if self.component != "am" and self.env.get("CD_LAYER_CACHE_WRITER_FLOOR") == "verified":
                try:
                    (self.layer_root / "replay.json").unlink(missing_ok=True)
                    if pending.exists():
                        shutil.rmtree(pending)
                    args += ["--cache-to", "type=local,mode=min,image-manifest=true,oci-mediatypes=true,ignore-error=true,dest=" + str(pending)]
                    self.layer_export_requested = True
                except OSError:
                    # The ordinary build can continue with its available reads.
                    pass
        if self.key is not None:
            args += ["--build-arg", "BUILDKIT_CACHE_MOUNT_NS=" + self.namespace]
        return args

    def exports_layers(self):
        # The legacy remote layer export is permanently read-only. Local
        # archives are verified and admitted separately after publication.
        return False

    def record_layer_replay(self, reservation):
        if not self.layer_export_requested or not (self.layer_root / "pending").is_dir():
            return
        # The exact successful build and its cache mount name are needed for
        # a clean-builder replay. Failure only withholds the optional cache.
        replay = self.layer_root / "replay.json"
        try:
            with tempfile.TemporaryDirectory(dir=self.layer_root) as directory:
                candidate = Path(directory) / "replay.json"
                candidate.write_text(json.dumps({"reservation": reservation,
                                                 "mount_namespace": self.namespace if self.key else None}))
                candidate.replace(replay)
        except OSError:
            pass

    def build_environment(self):
        # Buildx's cache client needs these; build steps, registry transport and
        # scanners never receive them as build arguments, secrets or mounts.
        if not all(self.env.get(key) for key in RUNTIME_KEYS):
            return {}
        return {key: self.env[key] for key in RUNTIME_KEYS}

    def can_overlap_export(self, archive):
        # The scan is complete. Leave room for the BuildKit stage copies, the
        # local bundle, and another archive-sized copy during registry work.
        try:
            required = archive.stat().st_size + 3 * MAX_BUNDLE_BYTES
            return shutil.disk_usage(self.root).free >= required
        except OSError:
            return False

    def reset_export(self):
        self.export_reason = None
        self.export_headroom = None
        self.export_retained_groups = None

    def accept_export(self, candidate):
        require(candidate.is_dir() and not candidate.is_symlink(), "Cargo cache candidate is invalid")
        tool = bridge()
        compacted = candidate / "compacted"
        if compacted.exists():
            groups = int(compacted.read_text())
            require(0 < groups <= tool.MAX_MEMBERS, "invalid compacted cache group count")
            self.export_retained_groups = groups
            compacted.unlink()
        tool.seal(candidate, self.key.prefix, family=self.key.family,
                  max_archive=MAX_BUNDLE_BYTES, max_content=MAX_EXPANDED_BYTES)
        size = sum((candidate / name).lstat().st_size for name in ("registry.tar.gz", "target.tar.gz"))
        self.export_headroom = "low" if size > HEADROOM_WARNING * MAX_BUNDLE_BYTES else "ok"
        if self.accepted.exists():
            require(not self.accepted.is_symlink(), "Build cache accepted root is invalid")
            shutil.rmtree(self.accepted)
        candidate.rename(self.accepted)
        return True

    def export_mounts(self):
        # Refresh after exact hits too. A single daily bundle makes later builds
        # recompile everything merged since that seed.
        self.reset_export()
        if self.key is None:
            return False
        require(not self.pending.exists(), "Cargo cache export must be fresh")
        try:
            tool = bridge()
            tool.transfer("extract", self.pending, namespace=self.namespace,
                          timeout=TRANSFER_SECONDS, max_content=MAX_EXPANDED_BYTES,
                          compact_registry=True, max_archive=MAX_BUNDLE_BYTES)
            return self.accept_export(self.pending)
        except EXPORT_ERRORS as error:
            self.export_reason = withheld_reason(error)
            return False
        finally:
            if self.pending.exists() and not self.pending.is_symlink():
                shutil.rmtree(self.pending)

    @contextmanager
    def background_export(self, directory):
        if self.key is None:
            yield None
            return
        self.reset_export()
        candidate = directory / "cargo-candidate"
        deadline = time.monotonic() + TRANSFER_SECONDS
        try:
            process = bridge().start_extract(candidate, directory / "cargo-context", namespace=self.namespace,
                                             max_content=MAX_EXPANDED_BYTES, max_archive=MAX_BUNDLE_BYTES)
        except EXPORT_ERRORS as error:
            self.export_reason = withheld_reason(error)
            yield lambda: False
            return

        def finish():
            try:
                status = process.wait(timeout=max(0, deadline - time.monotonic()))
                if status:
                    self.export_reason = "transfer-failed"
                    return False
                return self.accept_export(candidate)
            except EXPORT_ERRORS as error:
                self.export_reason = withheld_reason(error)
                return False

        try:
            yield finish
        finally:
            if process.poll() is None:
                with suppress(OSError, subprocess.SubprocessError):
                    terminate(process, grace_seconds=0)


# Fixed vocabulary only: exception text, paths and sizes never reach the log.
WITHHELD_REASONS = (("compressed cache exceeds", "oversize-compressed"),
                    ("expanded cache exceeds", "oversize-expanded"),
                    ("empty mount", "empty"))


def withheld_reason(error):
    if isinstance(error, subprocess.SubprocessError):
        return "transfer-failed"
    if isinstance(error, ValueError):
        text = str(error)
        for marker, reason in WITHHELD_REASONS:
            if marker in text:
                return reason
    return "invalid"


# BuildKit restores and extracts take about 60-70 s per GiB-sized bundle; a timed
# out restore silently becomes a cold build, so leave room for 2x growth.
TRANSFER_SECONDS = 360
# Warn while there is still room, before a growing bundle starts being withheld.
HEADROOM_WARNING = 0.75


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
