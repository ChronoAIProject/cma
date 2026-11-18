"""Prepare detector evidence whose Actions cache producer is protected develop."""
import hashlib
import json
import os
import re
from pathlib import Path

from staging_cd_common import require
from staging_cd_versions import DEPLOYABLES


def context(env):
    require(env.get("GITHUB_REPOSITORY") == "ChronoAIProject/cma"
            and env.get("GITHUB_EVENT_NAME") == "push"
            and env.get("GITHUB_REF") == "refs/heads/develop",
            "Detector cache requires protected develop publication")
    require(env.get("CD_COMPONENT") in DEPLOYABLES
            and re.fullmatch(r"[a-f0-9]{40}", env.get("GITHUB_SHA", "")),
            "Detector cache identity is invalid")
    for name in ("GITHUB_RUN_ID", "GITHUB_RUN_ATTEMPT"):
        require(re.fullmatch(r"[1-9][0-9]*", env.get(name, "")), "Detector cache run is invalid")
    temporary = Path(env["RUNNER_TEMP"]).resolve(strict=True)
    root = temporary / "cma-detector-cache"
    require(not root.is_symlink(), "Detector cache path is invalid")
    return root


def prepare(root, env):
    directory = context(env)
    directory.mkdir(mode=0o700, exist_ok=False)
    scanner = root / ".github/scripts/image-secret-scanner"
    files = sorted(scanner.rglob("*.py")) + [scanner / "gitleaks.toml"]
    digest = hashlib.sha256()
    for path in files:
        require(not path.is_symlink(), "Detector cache policy must be regular")
        digest.update(path.relative_to(scanner).as_posix().encode() + b"\0" + path.read_bytes())
    prefix = f"cma-cd-detector-v1-{env['CD_COMPONENT']}-{digest.hexdigest()}-"
    # The prefix separates compatibility; GitHub's actual ref authorization
    # provides trust. Develop cannot restore the PR merge-ref cache namespace.
    return {"directory": str(directory), "prefix": prefix,
            "key": prefix + env["GITHUB_RUN_ID"] + "-" + env["GITHUB_RUN_ATTEMPT"]}


def options(env):
    if env.get("CD_DETECTOR_CACHE") != "enabled":
        return {}
    directory = context(env)
    source = directory / "detector-cache.json"
    require(not source.is_symlink(), "Detector cache must be regular")
    return {"detector_cache": source if source.exists() else None,
            "detector_cache_output": source}


def main():
    values = prepare(Path.cwd(), os.environ)
    with Path(os.environ["GITHUB_OUTPUT"]).open("a") as stream:
        for key, value in values.items():
            stream.write(f"{key}={value}\n")
    print(json.dumps({"event": "detector-cache", "status": "prepared"}))


if __name__ == "__main__":
    main()
