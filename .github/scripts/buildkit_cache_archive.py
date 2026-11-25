"""Seal and restore a bounded, complete local BuildKit OCI cache."""

import argparse
import hashlib
import json
import os
import re
import shutil
import stat
import tarfile
from graphlib import TopologicalSorter
from pathlib import Path
from tempfile import TemporaryDirectory


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
    require(index.get("schemaVersion") == 2 and index.get("mediaType") == "application/vnd.oci.image.index.v1+json"
            and isinstance(index.get("manifests"), list) and len(index["manifests"]) == 1,
            "cache root must select one OCI manifest")
    require(index["manifests"][0].get("annotations", {}).get("org.opencontainers.image.ref.name") == "latest",
            "cache root does not select the default import")

    def descriptor_path(descriptor, media):
        name = descriptor.get("digest")
        require(isinstance(name, str) and DIGEST.fullmatch(name), "invalid cache descriptor digest")
        path = "blobs/sha256/" + name[7:]
        require(descriptor.get("mediaType") == media and type(descriptor.get("size")) is int
                and files.get(path) == descriptor["size"], "cache descriptor type, size or identity differs")
        return path

    manifest_path = descriptor_path(index["manifests"][0], "application/vnd.oci.image.manifest.v1+json")
    manifest = metadata(root / manifest_path)
    require(manifest.get("schemaVersion") == 2 and manifest.get("mediaType") == "application/vnd.oci.image.manifest.v1+json",
            "unsupported cache manifest")
    config_path = descriptor_path(manifest["config"], "application/vnd.buildkit.cacheconfig.v0")
    config = metadata(root / config_path)
    from actions_cache_buildkit_inventory import layers
    referenced = {"blobs/sha256/" + key.rsplit(":", 1)[1] for key in layers(config)}
    require(referenced and isinstance(manifest.get("layers"), list), "cache configuration has no usable layers")
    available = set()
    for descriptor in manifest["layers"]:
        media = descriptor.get("mediaType")
        require(isinstance(media, str) and (media.startswith("application/vnd.oci.image.layer.")
                or media.startswith("application/vnd.docker.image.rootfs.diff.")), "unsupported cache layer media type")
        available.add(descriptor_path(descriptor, media))
    require(referenced <= available, "cache configuration references an absent layer")

    records = config.get("records", [])
    require(records and any(record.get("layers") or any(chain.get("layers") for chain in record.get("chains", []))
                            for record in records), "cache configuration contains no result records")
    record_inputs = {}
    for index, record in enumerate(records):
        record_inputs[index] = set()
        require(re.fullmatch(DIGEST, record.get("digest", "")), "invalid cache record digest")
        inputs = record.get("inputs", [])
        require(isinstance(inputs, list) and all(isinstance(group, list) and group for group in inputs),
                "invalid cache record inputs")
        for group in inputs:
            for link in group:
                require(isinstance(link, dict) and type(link.get("link", 0)) is int
                        and 0 <= link.get("link", 0) < len(records), "unresolved cache record input")
                require(isinstance(link.get("selector", ""), str), "invalid cache input selector")
                record_inputs[index].add(link.get("link", 0))
    # BuildKit recursively resolves record inputs and layer parents. A
    # hash-valid graph with in-range cycles fails the supported importer.
    TopologicalSorter(record_inputs).prepare()
    parents = {index: set() if layer.get("parent", 0) == -1 else {layer.get("parent", 0)}
               for index, layer in enumerate(config["layers"])}
    TopologicalSorter(parents).prepare()


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
    # The final path is the publication boundary for the validated archive.
    # An interruption may leave temporary files, but no consumer can import
    # them through the final path before all checks complete.
    with TemporaryDirectory(prefix=".layer-restore-", dir=root.parent) as temporary:
        extracted = Path(temporary) / "validated"
        extracted.mkdir(mode=0o700)
        names = set()
        total = 0
        with tarfile.open(archive, "r:") as source:
            for member in source:
                require(member.isreg() and member_name(member.name) and member.name not in names,
                        "unexpected archive member")
                names.add(member.name)
                total += member.size
                require(len(names) <= MAX_MEMBERS and total <= MAX_BYTES, "cache archive exceeds bound")
                path = extracted / member.name
                path.parent.mkdir(parents=True, exist_ok=True)
                with source.extractfile(member) as data, path.open("xb") as output:
                    shutil.copyfileobj(data, output, CHUNK)
        inventory(extracted)
        require(not root.exists() and not root.is_symlink(), "cache restore destination already exists")
        extracted.rename(root)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("mode", choices=("seal", "restore"))
    parser.add_argument("directory", type=Path)
    parser.add_argument("archive", type=Path)
    parser.add_argument("--replay", type=Path)
    args = parser.parse_args()
    try:
        if args.mode == "seal":
            seal(args.directory, args.archive)
            if args.replay is not None:
                from staging_cd_layer_reuse import seal_replay
                seal_replay(args.replay, args.archive)
        else:
            restore(args.archive, args.directory)
    except (OSError, ValueError, TypeError, KeyError, AttributeError, tarfile.TarError) as error:
        print(f"BuildKit cache archive withheld ({type(error).__name__})")
        return 1
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
