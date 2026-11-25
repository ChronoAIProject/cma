"""Opt-in package profile for staging Agent Manager publication."""
from staging_cd_common import require
from staging_cd_versions import REPOSITORY

AM_PROFILE = "CMA_STAGING_AM_OPT_LEVEL"
AM_PROFILE_LABEL = "org.chronoai.cma.staging-am-opt-level"
AM_RECIPE = "api/crates/cma/Dockerfile"


def from_environment(env, component):
    level = env.get(AM_PROFILE, "") if component == "am" else ""
    require(level in ("", "0", "1"), "AM staging optimization level must be empty, 0 or 1")
    if level:
        require(env.get("GITHUB_REPOSITORY") == REPOSITORY
                and env.get("GITHUB_REF") == "refs/heads/develop"
                and env.get("GITHUB_EVENT_NAME") == "push",
                "AM staging profile requires a protected staging publication")
    return level


def for_source(root, component, level):
    require(level in ("", "0", "1") and (not level or component == "am"),
            "AM staging optimization level must be empty, 0 or 1")
    # A reservation made before the recipe gained the argument uses the
    # original profile. Its OCI identity and cache keys must say the same.
    if level and f"ARG {AM_PROFILE}" in (root / AM_RECIPE).read_text().splitlines():
        return level
    return ""
