"""The fixed deployment targets that the shared CD helpers act for.

Staging deploys from develop with the staging controller. Production deploys
from main with the production controller in cma-infra prod/chrono-cd. A job
selects a target with CD_TARGET; absent means staging, so staging callers are
unchanged. There is no free-form target: an unknown name is refused.
"""

from staging_cd_common import require

STAGING = "staging"
PRODUCTION = "production"
TARGETS = {
    STAGING: {
        "branch": "develop",
        "controller": ("staging",),
        "workflows": {"push": "staging-cd.yml", "workflow_dispatch": "redeploy-staging.yml"},
    },
    PRODUCTION: {
        "branch": "main",
        "controller": ("prod", "chrono-cd"),
        "workflows": {"push": "prod-cd.yml", "workflow_dispatch": "prod-cd.yml"},
    },
}


def name(env):
    """The selected target's name; absent means staging."""
    value = env.get("CD_TARGET", STAGING)
    require(value in TARGETS, "Deployment target is unknown")
    return value


def current(env):
    return TARGETS[name(env)]


def branch(env):
    return current(env)["branch"]


def controller(env):
    """Path parts of the private controller directory inside the cma-infra checkout."""
    return current(env)["controller"]


def workflows(env):
    return dict(current(env)["workflows"])
