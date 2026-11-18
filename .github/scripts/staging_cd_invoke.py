#!/usr/bin/env python3
"""Run one private coordinator with a private release file and bounded lifetime."""
import os
import subprocess
import sys
import tempfile
from pathlib import Path

from staging_cd_admission import event_identity
from staging_cd_checkout import checkout_path
from staging_cd_clock import elapsed, remaining, TOTAL_SECONDS, TERMINATION_GRACE_SECONDS
from staging_cd_common import Refusal, matches, require
from staging_cd_policy import selected as deploy_selection
from staging_cd_process import ProcessTimeout, run as run_process
from staging_cd_receipts import canonical
from staging_cd_runtime import run_private
from staging_cd_set import document, private_document, release_set, validate_set
from staging_cd_tools import environment as tool_environment

IDENTITY_KEYS = ("GITHUB_REPOSITORY", "GITHUB_REF", "GITHUB_EVENT_NAME",
                 "GITHUB_RUN_ID", "GITHUB_RUN_ATTEMPT")
PROCESS_KEYS = ("PATH", "LANG", "LC_ALL", "RUNNER_TEMP")
DISABLED = 100
RESULTS = {0: "PASS: deployment set public checks verified",
           DISABLED: "SKIP: deploy disabled in cma-infra",
           10: "FAIL: release failed; eligible owned changes recovered and verified",
           20: "FAIL: deployment unverified; immediate operator intervention required",
           30: "FAIL: deployment not admitted; no mutation attempted",
           40: "FAIL: deployment input or configuration error"}


class NotAdmitted(Refusal):
    """No private process has been permitted to mutate."""


def private_script(env, run_command=subprocess.run):
    revision = env.get("CD_INFRA_REF")
    require(matches(r"[0-9a-f]{40}", revision), "Private checkout must use an immutable commit")
    root = checkout_path(env)
    require(root.is_dir(), "Private checkout is absent")
    current = root
    for part in ("staging", "scripts", "deploy_set.py"):
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



def execute(script, release_file, mode, started, env, *, timeout=None):
    # SIGTERM at T+600 gives the controller up to 15 seconds to terminate/reap
    # the independent helper process groups it owns. This is no new rollout time.
    left = remaining(started, TOTAL_SECONDS)
    wait = min(left, timeout) if timeout is not None else left
    args = [sys.executable, "-B", str(script), "--release-set", str(release_file),
            "--mode", mode, "--started-at", started]
    try:
        code = run_process(args, env, wait, cwd=script.parents[2], grace_seconds=TERMINATION_GRACE_SECONDS)
    except ProcessTimeout as error:
        return 30 if error.returncode == 30 else 20
    except (subprocess.TimeoutExpired, KeyboardInterrupt):
        return 20
    if elapsed(started) > TOTAL_SECONDS and code != 30:
        return 20
    if mode == "bootstrap" and code == 10:
        return 20
    return code if code in (0, 10, 20, 30, 40) else 20


def deploy(env, masker, script_reader=private_script, runner=execute,
           tool_reader=tool_environment, policy_reader=deploy_selection, on_disabled=lambda: None):
    event_identity(env)
    value = validate_set(document(env.get("CD_RELEASE_SET")))
    mode, started = env.get("CD_DEPLOY_MODE"), env.get("CD_STARTED_AT")
    automatic = env["GITHUB_EVENT_NAME"] == "push"
    require(mode in ("release", "bootstrap") and (not automatic or mode == "release"),
            "Deployment mode is invalid")
    require(mode != "bootstrap" or set(value["receipts"]) == {"am", "eg", "web"},
            "Bootstrap requires all components")
    credentials = {key: env.get(key) for key in ("CD_NYXID_CLIENT_ID", "CD_NYXID_CLIENT_SECRET")}
    for credential in credentials.values():
        masker(credential)
    try:
        remaining(started)
        script = script_reader(env)
        selected = policy_reader(script, value["receipts"], automatic=automatic, mode=mode)
        if set(selected) != set(value["receipts"]):
            on_disabled()
        if not selected:
            return DISABLED
        require(all(isinstance(value, str) and bool(value) and not any(c.isspace() for c in value)
                    for value in credentials.values()),
                "Deployment credentials are missing or malformed")
    except Refusal as error:
        raise NotAdmitted("Deployment prerequisites are not satisfied") from error
    child_env = {key: env[key] for key in (*IDENTITY_KEYS, *PROCESS_KEYS) if key in env}
    child_env.update(tool_reader(env))
    child_env.update(credentials)
    # A set has independent source SHAs; the file is the sole source authority.
    # App tokens, registry credentials, keys and Actions command files stay out.
    selected_set = release_set({key: value["receipts"][key] for key in selected})
    with tempfile.TemporaryDirectory(prefix="cma-release-set-", dir=env["RUNNER_TEMP"]) as directory:
        path = Path(directory) / "release-set.json"
        with path.open("x", encoding="utf-8") as stream:
            os.chmod(path, 0o600)
            stream.write(canonical(private_document(selected_set)))
        try:
            remaining(started)
        except Refusal as error:
            raise NotAdmitted("Deployment setup exhausted admission") from error
        try:
            code = runner(script, path, mode, started, child_env)
        except Exception:  # noqa: BLE001 - a launched controller may have mutated.
            code = 20
    if elapsed(started) > TOTAL_SECONDS and code != 30:
        return 20
    return code if code in (0, 10, 20, 30, 40) else 20


def main():
    outcome, skipped = 40, False
    try:
        entry_elapsed = elapsed(os.environ.get("CD_STARTED_AT"))
    except (ValueError, TypeError, OverflowError):
        entry_elapsed = None
    def disabled():
        nonlocal skipped
        skipped = True
    def operation(masker):
        nonlocal outcome
        try:
            outcome = deploy(os.environ, masker, on_disabled=disabled)
        except NotAdmitted:
            outcome = 30
        return outcome in (0, DISABLED)
    result = run_private(operation, None)
    if skipped:
        print(RESULTS[DISABLED])
    if outcome == 20:
        print("::error::Staging deployment is unverified. Immediate operator intervention required.")
    summary = os.environ.get("GITHUB_STEP_SUMMARY")
    if summary:
        with open(summary, "a", encoding="utf-8") as stream:
            if skipped and outcome != DISABLED:
                stream.write(RESULTS[DISABLED] + "\n")
            stream.write(RESULTS[outcome] + "\n")
            if outcome == 0:
                stream.write("NOT-YET: authenticated acceptance runs out-of-band\n")
            try:
                duration = elapsed(os.environ.get("CD_STARTED_AT"))
            except (ValueError, TypeError, OverflowError):
                pass  # An invalid timestamp is already a failed admission.
            else:
                if entry_elapsed is not None:
                    stream.write(f"Admission-to-controller-entry seconds: {entry_elapsed:.2f}\n")
                stream.write(f"Admission-to-result seconds: {duration:.2f}\n")
    return 0 if result == 0 else outcome if outcome in (10, 20, 30, 40) else 40


if __name__ == "__main__":
    raise SystemExit(main())
