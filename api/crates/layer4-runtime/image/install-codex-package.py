#!/usr/bin/env python3
"""Install the verified native producer into its matching npm vendor layout."""

import argparse
import hashlib
import json
from pathlib import Path
import shutil
import tarfile
import tempfile


EXECUTABLES = (
    "bin/codex",
    "bin/codex-code-mode-host",
    "codex-resources/bwrap",
    "codex-path/rg",
)
METADATA = "codex-package.json"
DIRECTORIES = {"bin", "codex-resources", "codex-path"}


def sha256(path):
    digest = hashlib.sha256()
    with path.open("rb") as source:
        for chunk in iter(lambda: source.read(1024 * 1024), b""):
            digest.update(chunk)
    return digest.hexdigest()


def install(archive, archive_sha, target, version, vendor, executable_shas):
    if len(executable_shas) != len(EXECUTABLES):
        raise ValueError("Incomplete Codex executable pins")
    if sha256(archive) != archive_sha:
        raise ValueError("Codex package archive digest mismatch")
    expected_metadata = {
        "layoutVersion": 1,
        "version": version,
        "target": target,
        "variant": "codex",
        "entrypoint": "bin/codex",
        "resourcesDir": "codex-resources",
        "pathDir": "codex-path",
    }
    if json.loads((vendor / METADATA).read_text()) != expected_metadata:
        raise ValueError("Codex npm vendor package does not match the producer")

    files = {*EXECUTABLES, METADATA}
    with tempfile.TemporaryDirectory(prefix="cma-codex-package-") as directory:
        prepared = Path(directory)
        seen = set()
        with tarfile.open(archive, "r:gz") as package:
            for member in package:
                name = member.name
                if name in seen:
                    raise ValueError("Duplicate Codex package entry")
                seen.add(name)
                if name in DIRECTORIES and member.isdir():
                    continue
                if name not in files or not member.isfile():
                    raise ValueError("Unexpected Codex package entry")
                destination = prepared / name
                destination.parent.mkdir(parents=True, exist_ok=True)
                with package.extractfile(member) as source, destination.open("wb") as output:
                    shutil.copyfileobj(source, output)
        if not files.issubset(seen):
            raise ValueError("Incomplete Codex native package")
        if json.loads((prepared / METADATA).read_text()) != expected_metadata:
            raise ValueError("Codex release metadata mismatch")
        for name, expected in zip(EXECUTABLES, executable_shas):
            if sha256(prepared / name) != expected:
                raise ValueError("Codex executable digest mismatch: " + name)

        # Retain the frozen npm launcher's same-version resources, including
        # voice support. Replace every executable in the reviewed fork package.
        for name, expected in zip(EXECUTABLES, executable_shas):
            destination = vendor / name
            shutil.copyfile(prepared / name, destination)
            destination.chmod(0o755)
            if sha256(destination) != expected:
                raise ValueError("Installed Codex executable digest mismatch: " + name)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("archive", type=Path)
    parser.add_argument("archive_sha")
    parser.add_argument("target", choices=(
        "x86_64-unknown-linux-musl", "aarch64-unknown-linux-musl",
    ))
    parser.add_argument("version")
    parser.add_argument("vendor", type=Path)
    parser.add_argument("executable_shas", nargs=len(EXECUTABLES))
    args = parser.parse_args()
    install(args.archive, args.archive_sha, args.target, args.version, args.vendor, args.executable_shas)


if __name__ == "__main__":
    main()
