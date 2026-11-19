#!/usr/bin/env python3
"""Run one private coordinator with a private release file and bounded lifetime."""
import os
import subprocess
import sys
import tempfile
from pathlib import Path

from staging_cd_admission import event_identity
from staging_cd_applied import configured_group
from staging_cd_checkout import checkout_path
from staging_cd_clock import elapsed, remaining, TOTAL_SECONDS, TERMINATION_GRACE_SECONDS
from staging_cd_common import Refusal, matches, require
from staging_cd_policy import selected as deploy_selection
from staging_cd_process import ProcessTimeout, run as run_process
from staging_cd_receipts import canonical
from staging_cd_runtime import run_private
from staging_cd_set import document, private_document, release_set, validate_set
from staging_cd_tools import environment as tool_environment
from staging_cd_compatibility import ContractPolicy, GROUP
from staging_cd_history import DevelopHistory
from staging_cd_watermark import eligible_receipts, record_withheld, require_current_versions
from staging_cd_deploy_result import read as read_component_result, retain as retain_component_result

IDENTITY_KEYS = ("GITHUB_REPOSITORY", "GITHUB_REF", "GITHUB_EVENT_NAME",
                 "GITHUB_RUN_ID", "GITHUB_RUN_ATTEMPT")
PROCESS_KEYS = ("PATH", "LANG", "LC_ALL", "RUNNER_TEMP")
DISABLED = 100
UNVERIFIED_COHORT = 51
PENDING_RUNTIME_CHECK = "PASS_PENDING_LEAD_RUNTIME_IMAGE_CHECK"
RUNTIME_NOTICE = "NOT-YET: runtime_image_id requires independent lead verification before final acceptance"
RESULTS = {0: PENDING_RUNTIME_CHECK,
           DISABLED: "SKIP: deploy disabled in cma-infra",
           UNVERIFIED_COHORT: "NOT-YET: contract digests are configured; the serving cohort is unverified",
           10: "FAIL: release failed; eligible owned changes recovered and verified",
           20: "FAIL: deployment unverified; immediate operator intervention required",
           30: "FAIL: deployment not admitted; no mutation attempted",
           40: "FAIL: deployment input or configuration error",
           50: "FAIL: qualified components passed controller checks; incompatible peers were withheld"}


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
    return code if code in (0, 10, 20, 30, 40, 50) else 20


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
    receipts = {key: value["receipts"][key] for key in selected}
    result = None
    unverified_cohort = False
    with tempfile.TemporaryDirectory(prefix="cma-release-set-", dir=env["RUNNER_TEMP"]) as directory:
        path = Path(directory) / "release-set.json"
        try:
            if automatic:
                # A successor can arrive during tool or controller setup. Keep
                # the remaining eligible peers inside this same mutation lock.
                transition = value.get("contract_transition")
                def current_subset(candidates):
                    return eligible_receipts(candidates, require_current_versions,
                                             lambda: ContractPolicy(DevelopHistory.from_head(env["GITHUB_SHA"])),
                                             transition=transition)
                configured = set()
                current = current_subset(receipts)
                if transition and GROUP & current.keys():
                    require(GROUP <= current.keys(), "Deployment policy split the contract group")
                    configured = configured_group(script, directory, current, started, child_env)
                    # An allocation may have landed during the live observation.
                    current = current_subset(current)
                record_withheld(env, receipts, current)
                # Matching defaults alone cannot prove which template the ready
                # pods serve. When any member must change, submit all three:
                # the controller composes one PATCH and omits unchanged defaults.
                unverified_cohort = configured == GROUP and GROUP <= current.keys()
                receipts = {key: receipt for key, receipt in current.items()
                            if not unverified_cohort or key not in GROUP}
                if not receipts:
                    return UNVERIFIED_COHORT
            request = private_document(release_set(receipts))
            with path.open("x", encoding="utf-8") as stream:
                os.chmod(path, 0o600)
                stream.write(canonical(request))
            remaining(started)
        except (OSError, ValueError, subprocess.SubprocessError) as error:
            raise NotAdmitted("Deployment admission changed before controller launch") from error
        try:
            code = runner(script, path, mode, started, child_env)
            result = read_component_result(path.with_name("component-result.json"), request, code)
        except Exception:  # noqa: BLE001 - a launched controller may have mutated.
            code = 20
    if elapsed(started) > TOTAL_SECONDS and code != 30:
        return 20
    if result is not None:
        try:
            retain_component_result(result, env)
            from staging_cd_deploy_result import emit
            emit(result, request, code, env)
        except (OSError, ValueError, KeyError, TypeError):
            return 20
    if code == 0 and unverified_cohort:
        return UNVERIFIED_COHORT
    return code if code in (0, 10, 20, 30, 40, 50) else 20


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
    if outcome == UNVERIFIED_COHORT:
        print(RESULTS[UNVERIFIED_COHORT])
    if outcome == 20:
        print("::error::Staging deployment is unverified. Immediate operator intervention required.")
    summary = os.environ.get("GITHUB_STEP_SUMMARY")
    if summary:
        with open(summary, "a", encoding="utf-8") as stream:
            if skipped and outcome != DISABLED:
                stream.write(RESULTS[DISABLED] + "\n")
            if outcome == 0:
                stream.write("NOT-YET: authenticated acceptance runs out-of-band\n")
                stream.write(RUNTIME_NOTICE + "\n")
            try:
                duration = elapsed(os.environ.get("CD_STARTED_AT"))
            except (ValueError, TypeError, OverflowError):
                pass  # An invalid timestamp is already a failed admission.
            else:
                if entry_elapsed is not None:
                    stream.write(f"Admission-to-controller-entry seconds: {entry_elapsed:.2f}\n")
                stream.write(f"Admission-to-result seconds: {duration:.2f}\n")
            stream.write(RESULTS[outcome] + "\n")
    if result == 0 and outcome == 0:
        # A fixed public state survives suppression without forwarding private output.
        print(RUNTIME_NOTICE)
        print(PENDING_RUNTIME_CHECK)
    if outcome == 50:
        # The artifact was reconstructed from validated request-bound fields;
        # only fixed component/reason tokens are reflected into the public log.
        value = document((Path(os.environ["RUNNER_TEMP"]) / "public-deployment/deployment.json").read_text())
        from staging_cd_contract import GROUP
        for component in sorted(GROUP):
            if value.get("components", {}).get(component, {}).get("state") == "withheld":
                print("FAIL component=" + component + " reason=deployed-compatibility-unqualified")
    return 0 if result == 0 else outcome if outcome in (10, 20, 30, 40, 50, UNVERIFIED_COHORT) else 40


if __name__ == "__main__":
    raise SystemExit(main())
