"""Reuse Cargo's workspace objects only with a compatible input and output receipt."""

import argparse
import hashlib
import json
import os
from pathlib import Path
import re
import shutil
import stat
import subprocess
import time

FORMAT = 1
SEED = ".cma-workspace-seed.json"
FINGERPRINT = Path("release/.fingerprint")
LIBRARY = re.compile(r"(?:lib)?([A-Za-z0-9_]+)-[0-9a-f]+\.(?:rlib|rmeta|d)")
PACKAGE_DIR = re.compile(r"([A-Za-z0-9_-]+)-[0-9a-f]+")
INCREMENTAL_DIR = re.compile(r"([A-Za-z0-9_]+)-[0-9a-z]+")
RELEASE_VARIABLES = ("CHRONO_BUILD", "CHRONO_BUILD_SHA")
UNCHANGED_MTIME = 946684800  # A fixed epoch for source files verified byte for byte.
# Linux/amd64 musl release audits bind the locked registry and host build
# graph, including its features, to the retained workspace call sites.
# A new seed cannot qualify an unaudited change to any of those inputs.
RELEASE_AUDITS = {
    "cma": (("cma", "cma-allocator-qualification", "cma-service-auth",
             "layer1-public", "layer2-core", "layer3-adapter"),
            "ff99c254162de36363f80d9d5f2ca9276707798575ebacd7ac795ab17cb31994"),
    "cmaeg": (("cmaeg", "cmaeg-core", "cma-allocator-qualification", "cma-service-auth"),
              "bed9dcdef1ee41e478ec033a5eb1c61f122eb30ea0532fd7532f76ab16094c70"),
}
COMPILER_ENV = (
    "AR", "CC", "CFLAGS", "CXX", "CXXFLAGS", "LDFLAGS", "RUSTC", "RUSTFLAGS",
    "RUSTC_WRAPPER", "RUSTC_WORKSPACE_WRAPPER", "CARGO_BUILD_TARGET",
    "CARGO_BUILD_RUSTFLAGS", "CARGO_ENCODED_RUSTFLAGS", "CARGO_INCREMENTAL",
)
# This build script's complete filesystem reads are in its own package.
# A changed or added build program needs fresh qualification before reuse.
SCRIPT_INPUTS = (
    "crates/layer3-adapter/build.rs",
    "crates/layer3-adapter/build/runtime_versions.rs",
)
SCRIPT_DIGESTS = (
    "ede7636145d454a9ce3a9f80c4a719a875b7731610b4562a9556d6b229631ee6",
    "2cd68bcf8f9b4fcd1d49edd40cf1fa2d8b5e5641367995d44eb72ec07035d0b2",
)


def require(ok, message):
    if not ok:
        raise ValueError(message)


def digest(file):
    hasher = hashlib.sha256()
    with file.open("rb") as readable:
        for block in iter(lambda: readable.read(1024 * 1024), b""):
            hasher.update(block)
    return hasher.hexdigest()


def inventory(root, *, skip=()):
    files = {}
    for directory, folders, names in os.walk(root, onerror=raise_error):
        base = Path(directory)
        if base == root:
            folders[:] = [folder for folder in folders if folder not in skip]
            names = [name for name in names if name not in skip]
        for folder in folders:
            require(not (base / folder).is_symlink(), "symlink directory in compiler inputs or outputs")
        for name in names:
            path = base / name
            mode = path.lstat().st_mode
            require(stat.S_ISREG(mode), "non-regular compiler input or output")
            files[path.relative_to(root).as_posix()] = [mode & 0o777, digest(path)]
    return files


def raise_error(error):
    raise error


def cargo_metadata(root):
    result = subprocess.run(
        ["cargo", "metadata", "--format-version", "1", "--locked", "--offline", "--no-deps"],
        cwd=root, capture_output=True, text=True, check=True,
    )
    return json.loads(result.stdout)


def workspace(root, metadata, files):
    packages = metadata["packages"]
    require(Path(metadata["workspace_root"]).resolve() == root
            and {package["id"] for package in packages} == set(metadata["workspace_members"]),
            "incomplete workspace metadata")
    directories = {}
    for package in packages:
        name = package["name"]
        directory = Path(package["manifest_path"]).parent.relative_to(root).as_posix()
        require(re.fullmatch(r"[A-Za-z0-9_-]+", name) and name not in directories.values()
                and directory not in directories and directory != ".", "ambiguous workspace package")
        require(not any(directory.startswith(other + "/") or other.startswith(directory + "/")
                        for other in directories), "nested workspace packages")
        directories[directory] = name
        for target in package["targets"]:
            if "custom-build" in target["kind"]:
                script = Path(target["src_path"]).relative_to(root).as_posix()
                require(script == SCRIPT_INPUTS[0] and
                        [files.get(path, [None, None])[1] for path in SCRIPT_INPUTS] == list(SCRIPT_DIGESTS),
                        "unqualified workspace build script")
            require("proc-macro" not in target["kind"], "unqualified workspace proc macro")
    reverse = {name: set() for name in directories.values()}
    for package in packages:
        for dependency in package["dependencies"]:
            if dependency["kind"] == "dev" or dependency.get("path") is None:
                continue
            relative = Path(dependency["path"]).relative_to(root).as_posix()
            require(relative in directories, "unclassified workspace path dependency")
            reverse[directories[relative]].add(package["name"])
    return directories, reverse


def rebuild_set(old, new, directories, reverse):
    # New paths can change module resolution before Cargo emits new dep-info.
    require(old.keys() == new.keys(), "workspace path set changed")
    changed = sorted(name for name in new if old[name] != new[name])
    affected = set()
    for path in changed:
        owners = [name for directory, name in directories.items() if path.startswith(directory + "/")]
        require(len(owners) == 1, "shared compiler input changed")
        affected.add(owners[0])
    todo = list(affected)
    while todo:
        for dependent in reverse[todo.pop()] - affected:
            affected.add(dependent)
            todo.append(dependent)
    return sorted(affected), changed


def tool_output(*command):
    return subprocess.run(command, capture_output=True, text=True, check=True).stdout


def audit_digest(files, entrypoint, packages):
    owned = tuple(f"crates/{package}/" for package in packages)
    inputs = {}
    for path, (mode, checksum) in files.items():
        if path == entrypoint or path.startswith("build_support/"):
            # The build helper is already bound by the compiler receipt.
            continue
        if (path.startswith("crates/") and not path.startswith(owned)
                and Path(path).name != "Cargo.toml"):
            continue
        inputs[path] = (mode & 0o111, checksum)
    serialized = json.dumps(inputs, sort_keys=True, separators=(",", ":")).encode()
    return hashlib.sha256(serialized).hexdigest()


def audited_entrypoint(prior, files, compiler, affected, changed):
    binary = compiler["binary"]
    packages, approved = RELEASE_AUDITS[binary]
    entrypoint = f"crates/{binary}/src/main.rs"
    if changed and (affected != [binary] or changed != [entrypoint]):
        return False
    rustc = compiler["rustc"].splitlines()
    return (
        "host: x86_64-unknown-linux-musl" in rustc
        and "commit-hash: 31fca3adb283cc9dfd56b49cdee9a96eb9c96ffd" in rustc
        and compiler["cargo"].strip() == "cargo 1.96.1 (356927216 2026-06-26)"
        and compiler["environment"] == {"CARGO_INCREMENTAL": hashlib.sha256(b"1").hexdigest()}
        and entrypoint in files and prior["files"][entrypoint][0] == files[entrypoint][0]
        and audit_digest(prior["files"], entrypoint, packages) == approved
        and audit_digest(files, entrypoint, packages) == approved
    )


def compiler_inputs(binary, files):
    return {
        "format": FORMAT,
        "binary": binary,
        "rustc": tool_output("rustc", "-vV"),
        "cargo": tool_output("cargo", "-V"),
        "native": sorted(tool_output("apk", "info", "-v").splitlines()) if shutil.which("apk") else [],
        # Image build steps control the environment. Cargo itself tracks the two
        # per-release values in the dep-info of the units that read them.
        "environment": {key: hashlib.sha256(os.environ[key].encode()).hexdigest()
                        for key in COMPILER_ENV if key in os.environ},
        "configuration": {path: signature for path, signature in files.items()
                          if Path(path).name in ("Cargo.toml", "Cargo.lock", "rust-toolchain.toml", "Dockerfile")
                          or path.startswith((".cargo/", "build_support/"))},
    }


def read_seed(path):
    require(path.is_file() and not path.is_symlink() and path.stat().st_size <= 8 * 1024**2,
            "workspace seed receipt missing")
    seed = json.loads(path.read_bytes())
    require(isinstance(seed, dict) and set(seed) == {"compiler", "files", "outputs", "producer"},
            "workspace seed receipt invalid")
    require(all(isinstance(value, dict) for value in seed.values()), "invalid workspace seed fields")
    return seed


def reset(target, *, allow_legacy=False):
    release = target / "release"
    if release.exists() or release.is_symlink():
        if not (allow_legacy and not release.is_symlink() and release.is_dir()
                and {path.name for path in release.iterdir()} == {"incremental"}):
            discard(release)
    interrupted = target / "workspace-incremental"
    if interrupted.exists():
        discard(interrupted)


def prepare(root, target, binary, plan):
    path = target / SEED
    try:
        prior = read_seed(path)
    except (OSError, ValueError) as error:
        print(f"Workspace cache miss: {error}", flush=True)
        prior = None
    # Invalidate the receipt before changing any output. An interrupted or
    # unsuccessful build must never certify a partially updated target.
    path.unlink(missing_ok=True)
    (target / (SEED + ".pending")).unlink(missing_ok=True)
    try:
        files = inventory(root, skip=(target.name,))
        directories, reverse = workspace(root, cargo_metadata(root), files)
        require(binary in directories.values(), "binary not in workspace")
        compiler = compiler_inputs(binary, files)
    except (OSError, ValueError, KeyError, TypeError, subprocess.CalledProcessError) as error:
        print(f"Workspace cache disabled: {error}", flush=True)
        reset(target)
        plan.unlink(missing_ok=True)
        return

    try:
        require(prior is not None and prior["compiler"] == compiler, "incompatible compiler inputs")
        require(prior["outputs"] == inventory(target, skip=(SEED, "CACHEDIR.TAG", ".rustc_info.json")),
                "workspace output inventory differs")
        affected, changed = rebuild_set(prior["files"], files, directories, reverse)
        release_changed = prior["producer"] != {name: os.environ.get(name) for name in RELEASE_VARIABLES}
        retain = bool(changed or release_changed)
        # Package ownership cannot account for opaque macro reads. A macro in
        # an upstream workspace dependency can read the service's entry point
        # or release stamp. Discard the entire uncertified workspace target.
        require(not retain or audited_entrypoint(prior, files, compiler, affected, changed),
                "unqualified workspace input change")
        # git archive stamps the entire checkout with the candidate commit
        # time. Now that equality is proven, its timestamps carry no freshness
        # information. Without normalization Cargo recompiles unchanged units.
        for name in files.keys() - set(changed):
            os.utime(root / name, (UNCHANGED_MTIME, UNCHANGED_MTIME))
        # Cargo also tracks include! inputs that live outside the owning crate.
        # Their mtimes must advance past every restored fingerprint, even when
        # a git archive uses an older timestamp for the new content.
        if changed:
            latest = max((file.stat().st_mtime_ns for file in (target / FINGERPRINT).rglob("*")), default=0)
            now = time.time_ns()
            require(now > latest, "workspace fingerprint clock is ahead")
            for name in changed:
                os.utime(root / name, ns=(now, now))
            require(all((root / name).stat().st_mtime_ns > latest for name in changed),
                    "source clock does not advance restored fingerprints")
        print("Workspace cache compatible. Invalidated: none", flush=True)
        if retain:
            print(f"Workspace cache retained {binary} library under its release audit", flush=True)
    except (OSError, ValueError, KeyError, TypeError) as error:
        print(f"Workspace cache fallback: {error}", flush=True)
        reset(target, allow_legacy=prior is None)

    plan.write_text(json.dumps({"compiler": compiler, "files": files, "packages": list(directories.values())}))


def retain_session(path):
    sessions = [item for item in path.glob("s-*")
                if not item.name.endswith("-working") and (item / "work-products.bin").is_file()]
    newest = max(sessions, key=lambda session: (session / "work-products.bin").stat().st_mtime_ns,
                 default=None)
    for item in path.glob("s-*"):
        if item != newest:
            discard(item)


def trim(target, names, binary):
    release = target / "release"
    libraries = {name.replace("-", "_") for name in names}
    for path in release.iterdir():
        if path.name in (".fingerprint", "build", "incremental"):
            for item in path.iterdir():
                pattern = INCREMENTAL_DIR if path.name == "incremental" else PACKAGE_DIR
                match = pattern.fullmatch(item.name)
                keep = {binary} if path.name == "incremental" else names | libraries
                if not match or match[1] not in keep:
                    discard(item)
                elif path.name == "incremental":
                    retain_session(item)
        elif path.name == "deps":
            for item in path.iterdir():
                match = LIBRARY.fullmatch(item.name)
                if not match or match[1] not in libraries:
                    discard(item)
        else:
            discard(path)


def discard(path):
    if path.is_dir() and not path.is_symlink():
        shutil.rmtree(path)
    else:
        path.unlink()


def hydrate_dependencies(source, target):
    # BusyBox cp -an skips an existing directory entirely. Descend into
    # overlapping directories so Chef objects reach the mounted target without
    # overwriting a verified workspace object or its fingerprint.
    for entry in source.iterdir():
        destination = target / entry.name
        exists = destination.exists() or destination.is_symlink()
        if entry.is_dir() and not entry.is_symlink():
            if exists:
                require(destination.is_dir() and not destination.is_symlink(),
                        "Chef directory conflicts with Cargo target")
                hydrate_dependencies(entry, destination)
            else:
                shutil.copytree(entry, destination, symlinks=True)
        elif not exists:
            shutil.copy2(entry, destination, follow_symlinks=False)


def finish(target, plan, binary):
    if not plan.exists():
        return
    state = json.loads(plan.read_text())
    require(state["files"] == inventory(target.parent, skip=(target.name,)),
            "compiler mutated source while collecting workspace outputs")
    try:
        trim(target, set(state.pop("packages")), binary)
        # Cargo's fingerprints bind resolved features, profile and compiler
        # flags. Keep them with the matching libraries and generated outputs.
        state["outputs"] = inventory(target, skip=(SEED, "CACHEDIR.TAG", ".rustc_info.json"))
        state["producer"] = {name: os.environ.get(name) for name in RELEASE_VARIABLES}
        temporary = target / (SEED + ".pending")
        temporary.write_text(json.dumps(state, sort_keys=True, separators=(",", ":")))
        temporary.replace(target / SEED)
    except (OSError, ValueError) as error:
        print(f"Workspace cache export unavailable ({type(error).__name__})", flush=True)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("operation", choices=("prepare", "finish"))
    parser.add_argument("binary", choices=("cma", "cmaeg"))
    parser.add_argument("--dependency-target", type=Path)
    root = Path.cwd().resolve()
    args = parser.parse_args()
    target = root / "target"
    plan = Path("/tmp/cma-workspace-inputs.json")
    if args.operation == "prepare":
        prepare(root, target, args.binary, plan)
        if args.dependency_target is not None:
            hydrate_dependencies(args.dependency_target, target)
    else:
        finish(target, plan, args.binary)


if __name__ == "__main__":
    main()
