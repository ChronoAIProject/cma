"""An untrusted cache becomes usable only after every byte matches source pins."""
import hashlib
import json
from pathlib import Path

from staging_cd_common import require


def wheels():
    return json.loads(Path(__file__).with_name("staging_cd_wheels.json").read_text())


def cache_path(env):
    root = Path(env["RUNNER_TEMP"]) / "cma-public-tool-cache"
    require(root.parent.is_absolute() and root.parent.is_dir() and not root.parent.is_symlink()
            and not root.is_symlink(), "Deployment cache path is invalid")
    root.mkdir(mode=0o700, exist_ok=True)
    return root


def cached(env, url, digest, fetch):
    path = cache_path(env) / digest
    require(not path.is_symlink(), "Deployment cache links are forbidden")
    if path.exists():
        require(path.is_file() and path.stat().st_size <= 128 * 1024 * 1024,
                "Deployment cache entry is invalid")
        data = path.read_bytes()
    else:
        data = fetch(url, digest)
    require(hashlib.sha256(data).hexdigest() == digest, "Deployment cache checksum differs")
    if not path.exists():
        with path.open("xb") as stream:
            stream.write(data)
    return data


def warm(env, binaries, fetch):
    entries = [*binaries, *((row["url"], row["sha256"]) for row in wheels())]
    root = cache_path(env)
    require(set(path.name for path in root.iterdir()) <= {digest for _, digest in entries},
            "Deployment cache contains unexpected entries")
    for url, digest in entries:
        cached(env, url, digest, fetch)
    return True
