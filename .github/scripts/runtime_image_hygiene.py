"""Inventory every runtime layer and reject private storage before publication."""

import argparse
import hashlib
import json
import os
from pathlib import Path, PurePosixPath
import subprocess
import sys
import tarfile
import tempfile
import time

from image_scanner_policy import verify_bundle
from runtime_image_hygiene_policy import (
    EMPTY_RING, HygieneRefusal, apply_layer, base_identity, final_filesystem,
    image_path, layer_entry, require,
)


def readers():
    # Reuse the reviewed descriptor/diff-ID verifier, without changing detector
    # policy or adding an exception to the complete secret scanner.
    verify_bundle(Path(__file__).resolve().parents[2])
    sys.path.insert(0, str(Path(__file__).resolve().parent / "image-secret-scanner"))
    from image_secrets.archive import ImageArchive
    from image_secrets.common import Budget, digest_file
    return ImageArchive, Budget, digest_file


def member_kind(member):
    if member.isfile():
        return "file"
    if member.isdir():
        return "directory"
    if member.issym():
        return "symlink"
    if member.islnk():
        return "hardlink"
    raise HygieneRefusal("unsupported_entry")


def verify_tar_trailer(tar, maximum, deadline):
    # tarfile stops at the first zero header. Require both end blocks and only
    # zero padding afterwards, so an appended archive cannot hide more entries.
    tar.fileobj.seek(tar.offset)
    count = 0
    while True:
        require(time.monotonic() <= deadline, "inventory_timeout")
        block = tar.fileobj.read(1024 * 1024)
        if not block:
            break
        count += len(block)
        require(count <= maximum and not any(block), "tar_trailer_content")
    require(count >= 1024 and count % 512 == 0, "tar_trailer_missing")


def inventory_layer(tar, state, base, index, budget, deadline):
    entries, whiteouts, opaque, names = {}, set(), set(), set()
    exceptions = 0
    for member in tar:
        require(time.monotonic() <= deadline, "inventory_timeout")
        budget.entry()
        require(not member.issparse(), "sparse_entry")
        path = image_path(member.name)
        require(path not in names, "duplicate_layer_path")
        names.add(path)
        kind = member_kind(member)
        name = PurePosixPath(path).name
        parent = str(PurePosixPath(path).parent)
        if name.startswith(".wh."):
            require(kind == "file" and member.size == 0, "invalid_whiteout")
            if name == ".wh..wh..opq":
                opaque.add(parent)
            else:
                target = name[4:]
                require(target not in {"", ".", ".."}, "invalid_whiteout")
                whiteouts.add(str(PurePosixPath(parent) / target))
            continue
        digest = None
        if path == EMPTY_RING and kind == "file" and member.size == 0:
            with tar.extractfile(member) as stream:
                data = stream.read(1)
            require(data == b"", "empty_ring_content")
            digest = hashlib.sha256(data).hexdigest()
        if layer_entry(base, index, path, kind, member.size, digest):
            exceptions += 1
        value = {"kind": kind, "size": member.size}
        if kind in {"symlink", "hardlink"}:
            require("\0" not in member.linkname, "unsafe_link")
            value["target"] = member.linkname
        entries[path] = value
    verify_tar_trailer(tar, budget.max_bytes, deadline)
    apply_layer(state, entries, whiteouts, opaque)
    return exceptions


def check_archive(archive, component, output):
    require(component in {"runtime", "runtime-omarchy"}, "unknown_component")
    require(not output.exists(), "output_exists")
    output.parent.mkdir(parents=True, exist_ok=True)
    ImageArchive, Budget, digest_file = readers()
    original = digest_file(archive)
    deadline = time.monotonic() + 900
    budget = Budget(20 * 1024 ** 3, 1_000_000)
    exception_count = 0
    with tempfile.TemporaryDirectory(prefix="runtime-layer-inventory-", dir=output.parent) as temporary:
        with ImageArchive(archive, Path(temporary), budget) as image:
            verify_tar_trailer(image.tar, budget.max_bytes, deadline)
            _, images = image.images()
            require(len(images) == 1, "image_count")
            item = images[0]
            require(item["config"].get("os") == "linux" and
                    item["config"].get("architecture") == "amd64", "image_platform")
            base = base_identity(component, item["diff_ids"])
            state = {}
            for index, (name, diff) in enumerate(zip(item["layers"], item["diff_ids"])):
                require(time.monotonic() <= deadline, "inventory_timeout")
                layer = image.layer(name, diff)
                budget.reserve(layer.stat().st_size)
                try:
                    with tarfile.open(layer, "r:") as tar:
                        exception_count += inventory_layer(tar, state, base, index, budget, deadline)
                finally:
                    layer.unlink()
            final_filesystem(state)
    require(exception_count <= 1, "duplicate_exception")
    require(digest_file(archive) == original, "archive_changed")
    receipt = {"schema": "cma-runtime-layer-hygiene-v1", "component": component,
               "archive_sha256": original, "config_digest": item["config_digest"],
               "gate_sha256": digest_file(Path(__file__)),
               "policy_sha256": digest_file(Path(__file__).with_name("runtime_image_hygiene_policy.py")),
               "layer_diff_ids": item["diff_ids"], "complete": True, "status": "PASS",
               "private_keyring_final_entries": 0, "package_cache_entries": 0,
               "machine_identity": "empty", "exceptions": []}
    if exception_count:
        receipt["exceptions"] = [{"base_digest": base, "layer": 0, "path": EMPTY_RING,
                                  "size": 0, "content_sha256": hashlib.sha256(b"").hexdigest()}]
    output.write_text(json.dumps(receipt, indent=2) + "\n")
    return receipt


def docker_archive(reference, destination):
    # Resolve once, then export the immutable ID. Tags cannot redirect the gate.
    result = subprocess.run(["docker", "image", "inspect", reference, "--format", "{{.Id}}"],
                            capture_output=True, text=True, check=True, timeout=30)
    identity = result.stdout.strip()
    require(len(identity) == 71 and identity.startswith("sha256:")
            and all(c in "0123456789abcdef" for c in identity[7:]), "image_identity")
    with destination.open("xb") as stream:
        subprocess.run(["docker", "image", "save", identity], stdout=stream,
                       stderr=subprocess.DEVNULL, check=True, timeout=300)


def main(argv=None):
    parser = argparse.ArgumentParser(description=__doc__)
    source = parser.add_mutually_exclusive_group(required=True)
    source.add_argument("--archive", type=Path)
    source.add_argument("--image")
    parser.add_argument("--component", required=True, choices=("runtime", "runtime-omarchy"))
    parser.add_argument("--output", required=True, type=Path)
    args = parser.parse_args(argv)
    try:
        if args.image:
            with tempfile.TemporaryDirectory(prefix="runtime-image-inventory-",
                                             dir=os.environ.get("RUNNER_TEMP")) as directory:
                archive = Path(directory) / "image.tar"
                docker_archive(args.image, archive)
                receipt = check_archive(archive, args.component, args.output)
        else:
            receipt = check_archive(args.archive, args.component, args.output)
        print(f"Runtime layer hygiene passed: layers={len(receipt['layer_diff_ids'])}, "
              f"empty_base_exceptions={len(receipt['exceptions'])}, final_private_entries=0")
        return 0
    except Exception as error:
        reason = str(error) if isinstance(error, HygieneRefusal) else "incomplete_inventory"
        print(f"Runtime layer hygiene refused: {reason}")
        return 1


if __name__ == "__main__":
    raise SystemExit(main())
