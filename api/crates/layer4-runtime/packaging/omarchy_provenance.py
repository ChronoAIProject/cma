"""Verify the exact approved Arch package and installed files before final copy."""

import hashlib
import io
import json
import subprocess
import tarfile
import urllib.request
from pathlib import Path

LIMIT = 16 * 1024 * 1024


def sha(data):
    return hashlib.sha256(data).hexdigest()


def validate_archive(data, expected):
    if len(data) > LIMIT or sha(data) != expected["package"]["archive_sha256"]:
        raise ValueError("Package archive differs")
    decoded = subprocess.run(
        ["zstd", "-dc"], input=data, capture_output=True, check=True, timeout=30
    ).stdout
    if len(decoded) > 32 * 1024 * 1024:
        raise ValueError("Package payload exceeds bound")
    wanted = {item["path"]: item["sha256"] for item in expected["files"]}
    wanted[".MTREE"] = expected["metadata"]["mtree_sha256"]
    with tarfile.open(fileobj=io.BytesIO(decoded)) as archive:
        for name, digest in wanted.items():
            matches = [
                item
                for item in archive.getmembers()
                if item.name.removeprefix("./") == name
            ]
            if len(matches) != 1 or not matches[0].isfile() or matches[0].size > LIMIT:
                raise ValueError("Package member identity differs")
            if sha(archive.extractfile(matches[0]).read()) != digest:
                raise ValueError("Package member digest differs")


def field(data, name):
    sections = data.split("\n\n")
    matches = [
        part.splitlines()[1:]
        for part in sections
        if part.splitlines() and part.splitlines()[0] == "%" + name + "%"
    ]
    if len(matches) != 1:
        raise ValueError("Package record is ambiguous")
    return matches[0]


def validate_installation(root, expected, *, require_archive_record=True):
    metadata = expected["metadata"]
    base = root / metadata["root"]
    if any(
        path.is_symlink()
        for path in (base, base / "desc", base / "files", base / "mtree")
    ):
        raise ValueError("Package record is not regular")
    description = (base / "desc").read_text()
    for name, key in (
        ("NAME", "name"),
        ("VERSION", "version"),
        ("ARCH", "architecture"),
    ):
        if field(description, name) != [expected["package"][key]]:
            raise ValueError("Installed package identity differs")
    owned = field((base / "files").read_text(), "FILES")
    if sha((base / "mtree").read_bytes()) != metadata["mtree_sha256"]:
        raise ValueError("Installed package mtree differs")
    record = expected["package"]["archive_sha256"] + "  " + metadata["archive_path"]
    if require_archive_record and (root / metadata["package_hashes"]).read_text().splitlines().count(record) != 1:
        raise ValueError("Installed archive record differs")
    for item in expected["files"]:
        path = root / item["path"]
        if (
            owned.count(item["path"]) != 1
            or path.is_symlink()
            or not path.is_file()
            or path.stat().st_size != item["bytes"]
            or sha(path.read_bytes()) != item["sha256"]
        ):
            raise ValueError("Installed vector file differs")


def record_verified_archive(root, expected, data):
    # A base image may already supply this package, leaving no cached download
    # in the later install stage. Prove the fresh official archive and every
    # installed identity before recording that download; never infer its hash.
    root = root.resolve(strict=True)
    validate_archive(data, expected)
    validate_installation(root, expected, require_archive_record=False)
    metadata = expected["metadata"]
    history = root / metadata["package_hashes"]
    archive = root / metadata["archive_path"].lstrip("/")
    for path in (history, archive):
        if any(parent.is_symlink() for parent in (path, *path.parents)):
            raise ValueError("Package provenance path is redirected")
    previous = history.read_text()
    record = expected["package"]["archive_sha256"] + "  " + metadata["archive_path"]
    rows = [line for line in previous.splitlines()
            if line.split() and line.split()[-1] == metadata["archive_path"]]
    if rows and rows != [record]:
        raise ValueError("Installed archive record conflicts")
    archive.parent.mkdir(parents=True, exist_ok=True)
    # Exclusive creation preserves any existing cache file for investigation.
    with archive.open("xb") as stream:
        stream.write(data)
    try:
        if sha(archive.read_bytes()) != expected["package"]["archive_sha256"]:
            raise ValueError("Recorded package download differs")
        if not rows:
            history.write_text(previous + ("" if not previous or previous.endswith("\n") else "\n")
                               + record + "\n")
        validate_installation(root, expected)
    finally:
        # The verified download is evidence, not a second shipped package copy.
        archive.unlink()


def main():
    try:
        expected = json.loads(
            Path(__file__).with_name("gnutls-omarchy-package.json").read_text()
        )
        with urllib.request.urlopen(expected["package"]["url"], timeout=60) as response:
            data = response.read(LIMIT + 1)
        record_verified_archive(Path("/"), expected, data)
    except Exception:  # noqa: BLE001 -- private image bytes and exceptions stay out of logs
        print("runtime-arch-package-provenance=FAIL")
        return 1
    print("runtime-arch-package-provenance=PASS")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
