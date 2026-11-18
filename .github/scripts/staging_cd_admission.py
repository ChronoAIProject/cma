"""Authorize protected develop releases and maintainer-only redeployment."""
import json
from pathlib import Path

from staging_cd_common import matches, require
from staging_cd_versions import REPOSITORY


def event_identity(env):
    require(env.get("GITHUB_REPOSITORY") == REPOSITORY and env.get("GITHUB_REF") == "refs/heads/develop"
            and env.get("GITHUB_EVENT_NAME") in ("push", "workflow_dispatch")
            and matches(r"[0-9a-f]{40}", env.get("GITHUB_SHA")),
            "Deployment requires an authorized develop event")
    event = json.loads(Path(env["GITHUB_EVENT_PATH"]).read_text())
    require(isinstance(event, dict) and event.get("ref") in ("develop", "refs/heads/develop")
            and isinstance(event.get("repository"), dict)
            and event["repository"].get("full_name") == REPOSITORY, "Deployment event identity differs")
    if env["GITHUB_EVENT_NAME"] == "push":
        require(event["ref"] == "refs/heads/develop" and event.get("after") == env["GITHUB_SHA"]
                and event.get("deleted") is False and event.get("forced", False) is False,
                "Deployment push identity differs")
    else:
        require(isinstance(event.get("inputs"), dict) and isinstance(event["inputs"].get("deployables"), str),
                "Deployment dispatch inputs are invalid")
    return event


def maintainer_dispatch(env, request):
    event = event_identity(env)
    require(env["GITHUB_EVENT_NAME"] == "workflow_dispatch" and isinstance(event.get("inputs"), dict),
            "Redeployment requires a maintainer dispatch")
    for key in ("GITHUB_ACTOR", "GITHUB_TRIGGERING_ACTOR"):
        actor = env.get(key)
        require(matches(r"[A-Za-z0-9][A-Za-z0-9-]{0,38}(?:\[bot\])?", actor),
                "Dispatch actor is invalid")
        permission = request("collaborators/" + actor + "/permission")
        require(isinstance(permission, dict) and permission.get("role_name") in ("admin", "maintain"),
                "Redeployment requires maintainer permission")
    return event["inputs"]
