"""Strip exact reviewed documentation ranges without editing runtime code."""
import argparse
import hashlib
import json
import subprocess
from pathlib import Path, PurePosixPath

from prune import anchored

LIMIT = 2 * 1024 * 1024


def sha(data):
    return hashlib.sha256(data).hexdigest()


def read(root, relative):
    name = PurePosixPath(relative)
    if not relative or name.is_absolute() or ".." in name.parts or name.as_posix() != relative:
        raise ValueError("Invalid documentation path")
    path = anchored(root, relative)
    if path.is_symlink() or not path.is_file() or path.stat().st_size > LIMIT:
        raise ValueError("Invalid documentation source")
    return path.read_bytes()


def stripped(data, entry):
    if len(data) != entry["bytes"] or sha(data) != entry["source_sha256"]:
        raise ValueError("Documentation source differs")
    edits = entry["remove"]
    if not isinstance(edits, list) or not 1 <= len(edits) <= 16:
        raise ValueError("Invalid documentation edits")
    previous, result = 0, []
    for edit in edits:
        start, end = edit["start"], edit["end"]
        if (type(start) is not int or type(end) is not int
                or not previous <= start < end <= len(data)
                or sha(data[start:end]) != edit["sha256"]):
            raise ValueError("Documentation edit differs")
        result.append(data[previous:start])
        previous = end
    result.append(data[previous:])
    result = b"".join(result)
    if sha(result) != entry["result_sha256"]:
        raise ValueError("Documentation result differs")
    return result


def verify_package(root, entry):
    package = entry["package"]
    if "metadata_path" in package:
        data = read(root, package["metadata_path"])
        value = json.loads(data)
        if (sha(data) != package["metadata_sha256"]
                or any(value.get(key) != package[key] for key in ("name", "version"))):
            raise ValueError("Documentation package differs")
    elif "architecture" in package:
        result = subprocess.run(
            ["dpkg-query", "--admindir=" + str(anchored(root, "var/lib/dpkg")),
             "-W", "-f=${Package}\n${Version}\n${Architecture}\n", package["name"]],
            capture_output=True, check=True, text=True, timeout=30,
        )
        if result.stdout.splitlines() != [package[key] for key in ("name", "version", "architecture")]:
            raise ValueError("Documentation package differs")
    elif package["name"] != "NyxID skill":
        raise ValueError("Unknown documentation package")
    # Copied skill documents have no package database. Their exact original
    # bytes bind the source archive already pinned and verified at acquisition.


def prepare(root, policy):
    root = root.resolve(strict=True)
    if policy.get("schema") != "runtime-documentation-strips-v1":
        raise ValueError("Invalid documentation policy")
    entries = policy["entries"]
    if (not isinstance(entries, list) or not 1 <= len(entries) <= 16
            or len({entry["path"] for entry in entries}) != len(entries)):
        raise ValueError("Invalid documentation inventory")
    changes = []
    for entry in entries:
        verify_package(root, entry)
        result = stripped(read(root, entry["path"]), entry)
        changes.append((anchored(root, entry["path"]), result))
    # Validate all originals before modifying any of the prepared filesystem.
    for path, result in changes:
        path.write_bytes(result)


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--desktop", choices=("0", "1"), default="0")
    args = parser.parse_args()
    try:
        policy = json.loads(Path(__file__).with_name("documentation-strips.json").read_text())
        prepare(Path("/"), policy)
        if args.desktop == "1":
            desktop = Path(__file__).with_name("desktop-documentation-strips.json")
            prepare(Path("/"), json.loads(desktop.read_text()))
    except (OSError, ValueError, TypeError, KeyError, subprocess.SubprocessError):
        print("runtime-documentation-strip=FAIL")
        return 1
    print("runtime-documentation-strip=PASS")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
