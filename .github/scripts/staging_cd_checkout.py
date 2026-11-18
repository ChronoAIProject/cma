#!/usr/bin/env python3
"""Prepare and clean a pinned private checkout without logging its content."""
import os
import shutil
import sys
from pathlib import Path

from staging_cd_app_metadata import resolve_app_commit
from staging_cd_common import matches, require
from staging_cd_runtime import run_private


def checkout_path(env):
    require(all(matches(r"[1-9][0-9]*", env.get(k)) for k in ("GITHUB_RUN_ID", "GITHUB_RUN_ATTEMPT")),
            "Workflow identity is invalid")
    workspace = Path(env["GITHUB_WORKSPACE"])
    require(workspace.is_absolute() and workspace.is_dir() and not workspace.is_symlink(),
            "Workspace root is invalid")
    current = workspace
    for part in (".github", ".cd-private", f"{env['GITHUB_RUN_ID']}-{env['GITHUB_RUN_ATTEMPT']}"):
        current = current / part
        require(not current.is_symlink(), "Private checkout links are forbidden")
    return current


def prepare_app(env, masker, metadata_reader=resolve_app_commit):
    require(not checkout_path(env).exists(), "Private checkout path already exists")
    reference = env.get("CD_INFRA_REF", "main")
    require(reference in ("", "main") or matches(r"[0-9a-f]{40}", reference),
            "Private data requires main or an explicit commit")
    revision = metadata_reader(env.get("CD_INFRA_TOKEN"), reference, masker)
    require(matches(r"[0-9a-f]{40}", revision)
            and (reference in ("", "main") or revision == reference), "Private checkout commit differs")
    if env.get("GITHUB_OUTPUT"):
        with open(env["GITHUB_OUTPUT"], "a", encoding="utf-8") as stream:
            stream.write("revision=" + revision + "\n")
    if env.get("GITHUB_STEP_SUMMARY"):
        with open(env["GITHUB_STEP_SUMMARY"], "a", encoding="utf-8") as stream:
            stream.write("Infrastructure revision: `" + revision + "`\n")
    return True


def cleanup(env, _masker):
    path = checkout_path(env)
    if path.exists():
        require(path.is_dir(), "Private checkout path is not a directory")
        shutil.rmtree(path)
    return True


def main():
    def operation(masker):
        if sys.argv[1:] == ["prepare-app"]:
            return prepare_app(os.environ, masker)
        if sys.argv[1:] == ["cleanup"]:
            return cleanup(os.environ, masker)
        return False
    return run_private(operation, None)


if __name__ == "__main__":
    raise SystemExit(main())
