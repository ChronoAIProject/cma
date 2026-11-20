"""Compare verified OCI layer descriptors without unpacking container filesystems."""

import argparse
import hashlib
import json
from pathlib import Path
import re
import tarfile


def require(value, message):
    if not value:
        raise ValueError(message)


def inventory(path):
    with tarfile.open(path) as archive:
        names = archive.getnames()
        require(len(names) == len(set(names)), "duplicate OCI archive member")

        def read(name, maximum=4 * 1024 * 1024):
            member = archive.getmember(name)
            require(member.isfile() and member.size <= maximum, "invalid OCI metadata")
            return archive.extractfile(member).read()

        def blob(descriptor, *, metadata=False):
            require(isinstance(descriptor, dict)
                    and re.fullmatch(r"sha256:[0-9a-f]{64}", descriptor.get("digest", ""))
                    and type(descriptor.get("size")) is int and descriptor["size"] >= 0,
                    "invalid OCI descriptor")
            name = "blobs/sha256/" + descriptor["digest"][7:]
            member = archive.getmember(name)
            require(member.isfile() and member.size == descriptor["size"], "OCI blob size differs")
            digest = hashlib.sha256()
            with archive.extractfile(member) as source:
                for chunk in iter(lambda: source.read(1024 * 1024), b""):
                    digest.update(chunk)
            require("sha256:" + digest.hexdigest() == descriptor["digest"], "OCI blob digest differs")
            return json.loads(read(name)) if metadata else None

        index = json.loads(read("index.json"))
        for _ in range(4):
            if "manifests" not in index:
                break
            require(index.get("schemaVersion") == 2 and len(index["manifests"]) == 1,
                    "expected one runnable image without attestations")
            index = blob(index["manifests"][0], metadata=True)
        require(index.get("schemaVersion") == 2 and isinstance(index.get("layers"), list)
                and bool(index["layers"]), "invalid OCI image manifest")
        config = blob(index["config"], metadata=True)
        require(config.get("architecture") == "amd64" and config.get("os") == "linux",
                "proof requires the native publication platform")
        diff_ids = config.get("rootfs", {}).get("diff_ids", [])
        require(len(diff_ids) == len(index["layers"]), "filesystem layer inventory differs")
        result = []
        for layer, diff_id in zip(index["layers"], diff_ids):
            blob(layer)
            require(re.fullmatch(r"sha256:[0-9a-f]{64}", diff_id), "invalid uncompressed digest")
            result.append({"digest": layer["digest"], "bytes": layer["size"], "diff_id": diff_id})
        return result


def compare(before, after, *, app_only=False):
    require(bool(before) and bool(after), "empty layer inventory")
    known = {layer["digest"] for layer in before}
    added = {layer["digest"]: layer["bytes"] for layer in after if layer["digest"] not in known}
    prefix = 0
    for old, new in zip(before, after):
        if old != new:
            break
        prefix += 1
    if app_only:
        require(len(before) == len(after) and prefix == len(after) - 1
                and len(added) == 1, "application edit changed reusable layers")
    return {"schema": "cma-runtime-layer-comparison-v1", "before": before, "after": after,
            "unchanged_prefix_layers": prefix,
            "unchanged_prefix_bytes": sum(layer["bytes"] for layer in after[:prefix]),
            "new_compressed_bytes": sum(added.values()), "new_layer_count": len(added)}


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("before", type=Path)
    parser.add_argument("after", type=Path)
    parser.add_argument("output", type=Path)
    parser.add_argument("--app-only", action="store_true")
    args = parser.parse_args()
    result = compare(inventory(args.before), inventory(args.after), app_only=args.app_only)
    args.output.write_text(json.dumps(result, indent=2) + "\n")
    print(f"PASS layers: {result['unchanged_prefix_layers']} reused; "
          f"{result['new_compressed_bytes']} new compressed bytes")


if __name__ == "__main__":
    main()
