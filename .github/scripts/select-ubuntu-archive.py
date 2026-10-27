#!/usr/bin/env python3
"""Use the official HTTPS archive for GitHub-hosted Ubuntu browser dependencies."""

from pathlib import Path
import re


AZURE_MIRROR = re.compile(
    rb"https?://azure\.archive\.ubuntu\.com/ubuntu(?=/|\s|$)"
)


def select_archive(apt_directory: Path) -> int:
    sources = [
        apt_directory / "sources.list",
        # Hosted runners also resolve mirror+file sources through these lists.
        apt_directory / "apt-mirrors.txt",
        apt_directory / "apt-mirrors-security.txt",
    ]
    for suffix in ("*.list", "*.sources"):
        sources.extend(sorted((apt_directory / "sources.list.d").glob(suffix)))
    changed = 0
    for source in sources:
        if not source.is_file():
            continue
        original = source.read_bytes()
        updated = AZURE_MIRROR.sub(b"https://archive.ubuntu.com/ubuntu", original)
        if updated != original:
            # Keep suites, components, keyrings and all APT verification options.
            source.write_bytes(updated)
            changed += 1
    return changed


if __name__ == "__main__":
    count = select_archive(Path("/etc/apt"))
    print(f"Selected official Ubuntu HTTPS archive in {count} source files.")
