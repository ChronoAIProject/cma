"""Tell a queued Agent Manager release whether a newer reservation already owns it.

Only one AM release runs at a time (`cma-staging-am-release`). When a queued
job finally starts, a newer develop push has usually reserved a higher version
already. This check runs before the expensive Cargo and detector restores so a
superseded job skips them. It is an optimization only: the publication step
repeats the same pre-build check and remains the authority for the outcome.
"""
import json
import os
import sys
from pathlib import Path

from staging_cd_admission import event_identity
from staging_cd_common import require
from staging_cd_github import GitHub
from staging_cd_publish import validated_tasks
from staging_cd_reservations import Reservations, ReservationSuperseded
from staging_cd_resolve import output as step_output


def owned(env, root, *, github=GitHub, reservations=Reservations):
    """True while this job may still build; False once a successor owns every reservation."""
    event_identity(env)
    require(env.get("CD_COMPONENT") == "am", "Ownership precheck is Agent Manager only")
    tasks = validated_tasks(json.loads(env["CD_TASKS"]), env)
    token = env.get("GH_TOKEN")
    authority = reservations(github(token), token, root)
    for mode, value in tasks:
        if mode != "publish":
            # A resumed receipt is already published; finishing it needs no build.
            return True
        try:
            authority.require_current(value)
        except ReservationSuperseded:
            continue
        return True
    return False


def main():
    env = os.environ
    superseded = not owned(env, Path.cwd())
    step_output(env, "superseded", "true" if superseded else "false")
    print("AM ownership precheck: " + ("superseded; skipping cache restores" if superseded else "current"))
    return 0


if __name__ == "__main__":
    sys.exit(main())
