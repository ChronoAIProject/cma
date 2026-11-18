"""Bind installed library bytes to the reviewed upstream distribution package."""
import hashlib
import io
import json
import subprocess
import tarfile
import tempfile
import urllib.request
from pathlib import Path

DESTINATION = Path("/usr/share/cma-runtime/package-provenance/gnutls.json")
URL = ("https://security.debian.org/debian-security/pool/updates/main/g/gnutls28/"
       "libgnutls30t64_3.8.9-3+deb13u4_amd64.deb")


def sha(data):
    return hashlib.sha256(data).hexdigest()


def validate_package(data, expected):
    if len(data) > 8 * 1024 * 1024 or sha(data) != expected["package"]["sha256"]:
        raise ValueError("Package digest differs")
    with tempfile.TemporaryDirectory() as directory:
        path = Path(directory) / "package.deb"
        path.write_bytes(data)
        result = subprocess.run(["ar", "p", str(path), "data.tar.xz"], capture_output=True,
                                check=True, timeout=30)
    if len(result.stdout) > 8 * 1024 * 1024:
        raise ValueError("Package payload exceeds bound")
    with tarfile.open(fileobj=io.BytesIO(result.stdout), mode="r:xz") as archive:
        members = [entry for entry in archive if entry.name.removeprefix("./") == expected["library"]["path"]]
        if len(members) != 1 or not members[0].isfile() or members[0].size > 8 * 1024 * 1024:
            raise ValueError("Package library identity differs")
        contents = archive.extractfile(members[0]).read()
    if sha(contents) != expected["library"]["sha256"]:
        raise ValueError("Package library digest differs")


def validate_installation(root, expected, installed):
    if installed.splitlines() != [expected["package"][field] for field in ("name", "version", "architecture")]:
        raise ValueError("Installed package identity differs")
    library = root / expected["library"]["path"]
    if (library.is_symlink() or not library.is_file() or library.stat().st_size > 8 * 1024 * 1024
            or sha(library.read_bytes()) != expected["library"]["sha256"]):
        raise ValueError("Installed library digest differs")


def main():
    try:
        expected = json.loads(Path(__file__).with_name("gnutls-package.json").read_text())
        with urllib.request.urlopen(URL, timeout=60) as response:
            package = response.read(8 * 1024 * 1024 + 1)
        validate_package(package, expected)
        installed = subprocess.run(["dpkg-query", "-W", "-f=${Package}\n${Version}\n${Architecture}\n", "libgnutls30t64"],
                                   capture_output=True, check=True, text=True, timeout=30)
        validate_installation(Path("/"), expected, installed.stdout)
        DESTINATION.parent.mkdir(parents=True, exist_ok=True)
        DESTINATION.write_text(json.dumps(expected, sort_keys=True, separators=(",", ":")) + "\n")
    except Exception:  # noqa: BLE001 -- package or subprocess diagnostics never enter build logs
        print("runtime-package-provenance=FAIL")
        return 1
    print("runtime-package-provenance=PASS")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
