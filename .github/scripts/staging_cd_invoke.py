#!/usr/bin/env python3
"""Invoke the reviewed private deploy entry point without public diagnostics."""
import os
import signal
import subprocess
import sys

from staging_cd_checkout import checkout_path
from staging_cd_admission import event_identity
from staging_cd_common import matches, require
from staging_cd_runtime import run_private
from staging_cd_versions import DEPLOYABLES

IDENTITY_KEYS = ("GITHUB_REPOSITORY", "GITHUB_REF", "GITHUB_SHA", "GITHUB_EVENT_NAME",
                 "GITHUB_RUN_ID", "GITHUB_RUN_ATTEMPT")
PROCESS_KEYS = ("PATH", "LANG", "LC_ALL", "RUNNER_TEMP")
PACKAGES = {component: DEPLOYABLES[component] for component in ("am", "eg", "web")}
RESULTS = {0: "PASS: deployed and verified", 10: "FAIL: deployment failed; rollback verified",
           20: "FAIL: rollback uncertain; immediate operator intervention required",
           30: "FAIL: deployment preflight refused", 40: "FAIL: deployment configuration error"}


def validate_source(env):
    event_identity(env)
    require(matches(r"[0-9a-f]{40}", env.get("CD_SOURCE_SHA")), "Deployment receipt source is invalid")


def private_script(env, run_command=subprocess.run):
    revision = env.get("CD_INFRA_REF")
    require(matches(r"[0-9a-f]{40}", revision), "Private checkout must use an immutable commit")
    root = checkout_path(env)
    require(root.is_dir(), "Private checkout is absent")
    current = root
    for part in ("staging", "scripts", "deploy.py"):
        current = current / part
        require(not current.is_symlink(), "Private entry point links are forbidden")
    require(current.is_file(), "Private deployment entry point is absent")
    child_env = {k: env[k] for k in PROCESS_KEYS if k in env}
    child_env.update(GIT_CONFIG_NOSYSTEM="1", GIT_CONFIG_GLOBAL=os.devnull, GIT_TERMINAL_PROMPT="0")
    for args, expected in ((["rev-parse", "HEAD"], revision),
                           (["status", "--porcelain", "--untracked-files=all", "--", "staging"], "")):
        result = run_command(["git", "-C", str(root), *args], env=child_env,
                             capture_output=True, text=True, timeout=30, check=False)
        require(result.returncode == 0 and result.stdout.strip() == expected,
                "Private checkout differs from the reviewed commit")
    return current


def execute(script, component, reference, env, timeout=1500):
    # Private scripts own rollout, smoke and rollback. Their diagnostics never
    # cross the public runner boundary, including inherited native output.
    with subprocess.Popen([sys.executable, "-B", str(script), component, reference],
                          cwd=script.parents[2], env=env, stdin=subprocess.DEVNULL,
                          stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL,
                          start_new_session=True) as process:
        try:
            return process.wait(timeout=timeout)
        except subprocess.TimeoutExpired:
            os.killpg(process.pid, signal.SIGTERM)
            try:
                process.wait(timeout=5)
            except subprocess.TimeoutExpired:
                os.killpg(process.pid, signal.SIGKILL)
                process.wait(timeout=5)
            return 1


def deploy(env, masker, script_reader=private_script, runner=execute):
    validate_source(env)
    component, reference = env.get("CD_COMPONENT"), env.get("CD_IMAGE_REFERENCE")
    credential = env.get("NYXID_CHRONO_CD_KEY")
    masker(credential)
    require(component in PACKAGES, "Deployment component is unknown")
    require(matches(r"ghcr\.io/chronoaiproject/" + PACKAGES[component] + r"@sha256:[0-9a-f]{64}", reference),
            "Deployment image must match the component's immutable repository")
    require(isinstance(credential, str) and bool(credential) and not any(c.isspace() for c in credential),
            "Deployment credential is missing or malformed")
    script = script_reader(env)
    child_env = {k: env[k] for k in (*IDENTITY_KEYS, *PROCESS_KEYS) if k in env}
    child_env["NYXID_CHRONO_CD_KEY"] = credential
    child_env["GITHUB_SHA"] = env["CD_SOURCE_SHA"]
    # App tokens, registry credentials and the App private key are never passed.
    return runner(script, component, reference, child_env) == 0


def main():
    outcome = 40
    def runner(*args):
        nonlocal outcome
        code = execute(*args)
        outcome = code if code in RESULTS else 20
        return outcome
    result = run_private(lambda masker: deploy(os.environ, masker, runner=runner), None)
    if outcome == 20:
        # GitHub's error annotation and failed job notify subscribed maintainers
        # without disclosing the private target, API response or smoke output.
        print("::error::Staging rollback is uncertain. Immediate operator intervention required.")
    summary = os.environ.get("GITHUB_STEP_SUMMARY")
    if summary:
        with open(summary, "a", encoding="utf-8") as stream:
            stream.write(RESULTS[outcome] + "\n")
    return result


if __name__ == "__main__":
    raise SystemExit(main())
