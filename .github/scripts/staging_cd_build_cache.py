"""Persist only the locally qualified BuildKit and Cargo export closure.

BuildKit's direct remote exporter streams before image qualification. Use its
local exporter followed by the ref-scoped Actions cache transport so no byte
leaves the runner before the exact cache payload passes the image scanner.
"""
import hashlib
import datetime
import importlib.util
import json
import os
import shutil
import subprocess
import tarfile
from pathlib import Path

from image_secret_gate import scan_staging_archive
from staging_cd_common import require
from staging_cd_cache_context import context

MAX_BYTES = 8 * 1024**3
MAX_FILES = 200000


def bridge():
    spec = importlib.util.spec_from_file_location("cargo_cache_bridge", Path(__file__).with_name("api-cargo-cache.py"))
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


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
    tools = [Path(__file__), Path(__file__).with_name("api-cargo-cache.py"),
             Path(__file__).with_name("image_scanner_policy.py")]
    payload = {"format": 1, "component": component, "platform": "linux/amd64",
               "profile": "release", "recipe": recipe,
               "refresh_day": datetime.datetime.now(datetime.timezone.utc).date().isoformat(),
               "inputs": source_inputs,
               "tools": {p.name: hashlib.sha256(p.read_bytes()).hexdigest() for p in tools}}
    return hashlib.sha256(json.dumps(payload, sort_keys=True).encode()).hexdigest()


def files(directory):
    require(directory.is_dir() and not directory.is_symlink(), "Build cache directory is invalid")
    result, size = {}, 0
    for path in sorted(directory.rglob("*")):
        require(not path.is_symlink(), "Build cache links are forbidden")
        if path.is_dir():
            continue
        require(path.is_file() and path.stat().st_nlink == 1, "Build cache special files are forbidden")
        size += path.stat().st_size
        require(size <= MAX_BYTES and len(result) < MAX_FILES, "Build cache exceeds bounded transfer")
        digest = hashlib.sha256()
        with path.open("rb") as stream:
            for block in iter(lambda: stream.read(1024 * 1024), b""):
                digest.update(block)
        result[path.relative_to(directory).as_posix()] = digest.hexdigest()
    require(result, "Empty build cache is not reusable")
    return result


def cache_archive(directory, target):
    """The wrapper treats every exported file, including discarded stages, as input."""
    import io
    payload = files(directory)
    layer = target.with_suffix(".layer")
    try:
        with tarfile.open(layer, "w") as archive:
            for name in payload:
                archive.add(directory / name, arcname=name, recursive=False)
        from image_secret_gate import checksum
        digest = checksum(layer)
        config = json.dumps({"architecture": "amd64", "os": "linux", "config": {},
                             "rootfs": {"type": "layers", "diff_ids": ["sha256:" + digest]}}).encode()
        config_name = hashlib.sha256(config).hexdigest() + ".json"
        manifest = json.dumps([{"Config": config_name, "RepoTags": [], "Layers": ["layer.tar"]}]).encode()
        with tarfile.open(target, "w") as archive:
            for name, data in ((config_name, config), ("manifest.json", manifest)):
                entry = tarfile.TarInfo(name)
                entry.size = len(data)
                archive.addfile(entry, io.BytesIO(data))
            archive.add(layer, arcname="layer.tar", recursive=False)
    finally:
        layer.unlink(missing_ok=True)
    return payload


class BuildCache:
    def __init__(self, root, env, source, component, scanner=scan_staging_archive):
        context(env)  # Authority failures are never downgraded to a cache miss.
        self.component, self.scanner = component, scanner
        self.root = Path(env["RUNNER_TEMP"]) / "cma-build-cache"
        require(not self.root.is_symlink(), "Build cache root is invalid")
        self.root.mkdir(mode=0o700, exist_ok=True)
        self.accepted = self.root / "accepted"
        self.pending = self.root / "pending"
        self.compatibility = identity(source, component)
        self.restored = False

    def restore(self):
        if not self.accepted.exists():
            return False
        try:
            require(self.accepted.is_dir() and not self.accepted.is_symlink(),
                    "Build cache accepted root is invalid")
            receipt = self.accepted / "receipt.json"
            require(receipt.is_file() and not receipt.is_symlink() and receipt.stat().st_size <= 32 * 1024**2,
                    "Build cache receipt is invalid")
            data = json.loads(receipt.read_text())
            require(set(data) == {"identity", "files"} and data["identity"] == self.compatibility
                    and data["files"] == files(self.accepted / "data"), "Build cache qualification differs")
            cargo = self.accepted / "data/cargo"
            require(self.component not in ("am", "eg") or cargo.is_dir(),
                    "Rust build cache is missing its Cargo mounts")
            if cargo.exists():
                tool = bridge()
                tool.validate_bundle(cargo, self.compatibility)
                tool.transfer("inject", cargo)
            self.restored = True
        except (OSError, ValueError, KeyError, TypeError, EOFError, tarfile.TarError, subprocess.SubprocessError):
            self.restored = False
        return self.restored

    def build_options(self):
        if self.restored:
            # Stable dependencies do not need a new upload for every application
            # edit. Keep qualification/transfer off the warm release path.
            return ["--cache-from", "type=local,src=" + str(self.accepted / "data/buildkit")]
        require(not self.pending.exists(), "Build cache export must be fresh")
        self.pending.mkdir(mode=0o700)
        return ["--cache-to", "type=local,mode=max,ignore-error=true,dest=" + str(self.pending / "buildkit")]

    def qualify(self):
        require(self.pending.is_dir(), "Build cache export is absent")
        archive, report = self.root / "qualification.tar", self.root / "qualification-report"
        try:
            require((self.pending / "buildkit/index.json").is_file(), "Build cache index is absent")
            if self.component in ("am", "eg"):
                tool = bridge()
                cargo = self.pending / "cargo"
                tool.transfer("extract", cargo)
                tool.seal(cargo, self.compatibility)
            inventory = cache_archive(self.pending, archive)
            self.scanner(archive, report)
            require(inventory == files(self.pending), "Build cache changed during qualification")
            if self.accepted.exists():
                require(not self.accepted.is_symlink(), "Build cache accepted root is invalid")
                shutil.rmtree(self.accepted)
            self.accepted.mkdir(mode=0o700)
            self.pending.rename(self.accepted / "data")
            (self.accepted / "receipt.json").write_text(json.dumps({"identity": self.compatibility, "files": inventory}))
            return True
        except (OSError, ValueError, KeyError, TypeError, EOFError, tarfile.TarError, subprocess.SubprocessError):
            # Acceleration is optional. An unqualified payload is never exported,
            # even when the independently scanned final image is acceptable.
            if self.accepted.exists() and not self.accepted.is_symlink():
                shutil.rmtree(self.accepted)
            return False
        finally:
            archive.unlink(missing_ok=True)
            if self.pending.exists() and not self.pending.is_symlink():
                shutil.rmtree(self.pending)
            if report.exists() and not report.is_symlink():
                shutil.rmtree(report)


def prepare(root, env):
    context(env)
    component = env["CD_COMPONENT"]
    require(component in ("am", "eg"), "Build caching is limited to reviewed Rust services")
    prefix = f"cma-cd-build-v1-{component}-{identity(root, component)}-"
    return {"directory": str(Path(env["RUNNER_TEMP"]) / "cma-build-cache/accepted"),
            "prefix": prefix, "key": prefix + env["GITHUB_RUN_ID"] + "-" + env["GITHUB_RUN_ATTEMPT"]}


if __name__ == "__main__":
    with Path(os.environ["GITHUB_OUTPUT"]).open("a") as output:
        for key, value in prepare(Path.cwd(), os.environ).items():
            output.write(f"{key}={value}\n")
