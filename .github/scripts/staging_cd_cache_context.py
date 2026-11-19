"""Ref-bound authority shared by publication cache operations."""
import re
from pathlib import Path

from staging_cd_common import require
from staging_cd_versions import DEPLOYABLES


def context(env):
    require(env.get("GITHUB_REPOSITORY") == "ChronoAIProject/cma"
            and env.get("GITHUB_EVENT_NAME") == "push"
            and env.get("GITHUB_REF") == "refs/heads/develop",
            "Build cache requires protected develop publication")
    require(env.get("CD_COMPONENT") in DEPLOYABLES
            and re.fullmatch(r"[a-f0-9]{40}", env.get("GITHUB_SHA", "")),
            "Build cache identity is invalid")
    for name in ("GITHUB_RUN_ID", "GITHUB_RUN_ATTEMPT"):
        require(re.fullmatch(r"[1-9][0-9]*", env.get(name, "")), "Build cache run is invalid")
    temporary = Path(env["RUNNER_TEMP"]).resolve(strict=True)
    root = temporary / "cma-build-cache"
    require(not root.is_symlink(), "Build cache path is invalid")
    return root
