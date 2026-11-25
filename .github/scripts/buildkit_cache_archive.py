"""Seal and restore a bounded, complete local BuildKit OCI cache."""

import argparse
import hashlib
import json
import os
import re
import shutil
import stat
import tarfile
from pathlib import Path


MAX_BYTES = 4 * 1024**3
MAX_MEMBERS = 4096
MAX_METADATA = 4 * 1024**2
CHUNK = 1024**2
METADATA = {"index.json", "oci-layout"}
BLOB = re.compile(r"blobs/sha256/[a-f0-9]{64}")
DIGEST = re.compile(r"sha256:[a-f0-9]{64}")


def require(condition, reason):
    if not condition:
        raise ValueError(reason)


def member_name(name):
    return name in METADATA or BLOB.fullmatch(name) is not None


def digest(stream):
    value = hashlib.sha256()
    for block in iter(lambda: stream.read(CHUNK), b""):
        value.update(block)
    return value.hexdigest()


def metadata(path):
    require(path.stat().st_size <= MAX_METADATA, "cache metadata exceeds bound")
    with path.open("rb") as stream:
        return json.load(stream)


def check_graph(root, files):
    require(metadata(root / "oci-layout") == {"imageLayoutVersion": "1.0.0"}, "cache layout version differs")
    index = metadata(root / "index.json")
    require(index.get("schemaVersion") == 2 and isinstance(index.get("manifests"), list)
            and index["manifests"], "cache index is incomplete")
    pending = list(index["manifests"])
    visited = set()
    while pending:
        descriptor = pending.pop()
        name = descriptor.get("digest")
        require(isinstance(name, str) and DIGEST.fullmatch(name), "invalid cache descriptor digest")
        path = "blobs/sha256/" + name[7:]
        require(type(descriptor.get("size")) is int and files.get(path) == descriptor["size"],
                "cache descriptor is absent or has a different size")
        media = descriptor.get("mediaType")
        identity = (name, media)
        if identity in visited:
            continue
        visited.add(identity)
        require(len(visited) <= MAX_MEMBERS, "cache descriptor count exceeds bound")
        if media in ("application/vnd.oci.image.index.v1+json", "application/vnd.docker.distribution.manifest.list.v2+json"):
            pending.extend(metadata(root / path)["manifests"])
        elif media in ("application/vnd.oci.image.manifest.v1+json", "application/vnd.docker.distribution.manifest.v2+json"):
            manifest = metadata(root / path)
            pending.extend([manifest["config"], *manifest["layers"]])
        elif media == "application/vnd.buildkit.cacheconfig.v0":
            from actions_cache_buildkit_inventory import layers
            referenced = layers(metadata(root / path))
            require(all("blobs/sha256/" + key.rsplit(":", 1)[1] in files for key in referenced),
                    "cache configuration references an absent layer")
        else:
            require(isinstance(media, str) and (media.startswith("application/vnd.oci.image.layer.")
                    or media.startswith("application/vnd.docker.image.rootfs.diff.")), "unsupported cache media type")


def inventory(root):
    require(root.is_dir() and not root.is_symlink(), "cache root is unavailable")
    files = {}
    total = 0
    for directory, dirs, names in os.walk(root, followlinks=False):
        for name in dirs:
            path = Path(directory) / name
            require(not path.is_symlink() and path.relative_to(root).as_posix() in {"blobs", "blobs/sha256", "ingest"},
                    "unexpected cache directory")
        for name in names:
            path = Path(directory) / name
            relative = path.relative_to(root).as_posix()
            info = path.lstat()
            # Buildx leaves its local exporter lock behind. No lock state
            # travels with the archive, and a restore opens its own lock.
            if relative == "cache.lock":
                require(stat.S_ISREG(info.st_mode) and info.st_nlink == 1 and info.st_size == 0,
                        "unexpected cache lock")
                continue
            require(member_name(relative) and stat.S_ISREG(info.st_mode) and info.st_nlink == 1,
                    "unexpected cache member")
            total += info.st_size
            files[relative] = info.st_size
            require(len(files) <= MAX_MEMBERS and total <= MAX_BYTES, "cache exceeds size or member bound")
            if BLOB.fullmatch(relative):
                with path.open("rb") as stream:
                    require(digest(stream) == path.name, "cache blob digest differs")
    require(METADATA <= files.keys(), "cache metadata is incomplete")
    check_graph(root, files)
    return files


def seal(root, archive):
    files = inventory(root)
    require(not archive.exists() and not archive.is_symlink(), "cache archive already exists")
    try:
        with tarfile.open(archive, "x", format=tarfile.USTAR_FORMAT) as output:
            for name, size in sorted(files.items()):
                member = tarfile.TarInfo(name)
                member.mode = 0o600
                member.size = size
                with (root / name).open("rb") as stream:
                    output.addfile(member, stream)
        require(archive.stat().st_size <= MAX_BYTES, "cache archive exceeds byte bound")
    except BaseException:
        archive.unlink(missing_ok=True)
        raise


def restore(archive, root):
    info = archive.lstat()
    require(stat.S_ISREG(info.st_mode) and info.st_nlink == 1 and 0 < info.st_size <= MAX_BYTES,
            "cache archive differs from storage bound")
    require(not root.exists() and not root.is_symlink(), "cache restore destination already exists")
    root.mkdir(mode=0o700)
    names = set()
    total = 0
    try:
        with tarfile.open(archive, "r:") as source:
            for member in source:
                require(member.isreg() and member_name(member.name) and member.name not in names,
                        "unexpected archive member")
                names.add(member.name)
                total += member.size
                require(len(names) <= MAX_MEMBERS and total <= MAX_BYTES, "cache archive exceeds bound")
                path = root / member.name
                path.parent.mkdir(parents=True, exist_ok=True)
                with source.extractfile(member) as data, path.open("xb") as output:
                    shutil.copyfileobj(data, output, CHUNK)
        inventory(root)
    except BaseException:
        shutil.rmtree(root)
        raise


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("mode", choices=("seal", "restore"))
    parser.add_argument("directory", type=Path)
    parser.add_argument("archive", type=Path)
    args = parser.parse_args()
    try:
        if args.mode == "seal":
            seal(args.directory, args.archive)
        else:
            restore(args.archive, args.directory)
    except (OSError, ValueError, TypeError, KeyError, AttributeError, tarfile.TarError) as error:
        print(f"BuildKit cache archive withheld ({type(error).__name__})")
        return 1
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
