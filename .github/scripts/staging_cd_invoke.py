#!/usr/bin/env python3
"""Run one private coordinator with a private release file and bounded lifetime."""
from contextlib import contextmanager
import os
import subprocess
import sys
import tempfile
from pathlib import Path

from ci_outcome import error_site, report
from staging_cd_admission import event_identity
from staging_cd_applied import configured_group
from staging_cd_checkout import checkout_path
from staging_cd_clock import (CONTROLLER_TOTAL_ENV, TERMINATION_GRACE_SECONDS, controller_seconds, elapsed, remaining)
from staging_cd_common import Refusal, matches, require
from staging_cd_policy import selected as deploy_selection
from staging_cd_process import ProcessTimeout, run as run_process
from staging_cd_receipts import canonical
from staging_cd_runtime import run_private
from staging_cd_refusal import (
    REASONS as PRIVATE_REASONS, UNRECOGNIZED, VERIFICATION_REASONS, read as read_refusal, read_verification,
)
from staging_cd_set import OVERTAKEN, document, private_document, release_set, validate_set
from staging_cd_tools import environment as tool_environment
from staging_cd_compatibility import ContractPolicy, GROUP
from staging_cd_history import DevelopHistory
from staging_cd_watermark import (SUPERSEDED, eligible_receipts, later_final_sources, record_withheld,
                                  require_current_versions)
from staging_cd_deploy_result import read as read_component_result, retain as retain_component_result

IDENTITY_KEYS = ("GITHUB_REPOSITORY", "GITHUB_REF", "GITHUB_EVENT_NAME",
                 "GITHUB_RUN_ID", "GITHUB_RUN_ATTEMPT")
PROCESS_KEYS = ("PATH", "LANG", "LC_ALL", "RUNNER_TEMP")
DISABLED = 100
SUPERSEDED_BY_NEWER_FINAL = 60
UNVERIFIED_COHORT = 51
CONTRACT_READ_FAILED = 52
PENDING_RUNTIME_CHECK = "PASS_PENDING_LEAD_RUNTIME_IMAGE_CHECK"
RUNTIME_NOTICE = "NOT-YET: runtime_image_id requires independent lead verification before final acceptance"
RESULTS = {0: PENDING_RUNTIME_CHECK,
           DISABLED: "SKIP: deploy disabled in cma-infra",
           SUPERSEDED_BY_NEWER_FINAL: f"SKIP: {OVERTAKEN}; a newer final's run owns this deployment",
           UNVERIFIED_COHORT: "NOT-YET: contract digests are configured; the serving cohort is unverified",
           CONTRACT_READ_FAILED: "FAIL: AM and runtimes withheld because configured digests could not be verified",
           10: "FAIL: release failed; eligible owned changes recovered and verified",
           20: "FAIL: deployment unverified; immediate operator intervention required",
           30: "FAIL: deployment not admitted; no mutation attempted",
           40: "FAIL: deployment input or configuration error",
           50: "FAIL: qualified components passed controller checks; incompatible peers were withheld"}


PRECONDITIONS = {"mode", "checkout", "policy", "credentials-format", "budget", "authority"}
# Outcome tokens the public log may carry; anything else stays "unreported".
REPORTED_PRIVATE = PRIVATE_REASONS | {UNRECOGNIZED}
REPORTED_VERIFICATION = VERIFICATION_REASONS | {UNRECOGNIZED}


class NotAdmitted(Refusal):
    """A public precondition stopped the invocation before the controller ran."""

    def __init__(self, reason, code):
        super().__init__("Deployment precondition was not admitted")
        self.reason = reason
        self.code = code


@contextmanager
def precondition(reason, refusal_code=30):
    try:
        yield
    except Refusal as error:
        raise NotAdmitted(reason, refusal_code) from error
    except Exception as error:
        # Unexpected setup errors retain the caller's configuration-error status.
        raise NotAdmitted(reason, 40) from error


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
    # Reserve the same absolute clock for the child. Older controllers ignore
    # this timing input and retain their shorter deadline.
    limit = controller_seconds(mode)
    child_env = {**env, CONTROLLER_TOTAL_ENV: str(limit)}
    left = remaining(started, limit)
    wait = min(left, timeout) if timeout is not None else left
    args = [sys.executable, "-B", str(script), "--release-set", str(release_file),
            "--mode", mode, "--started-at", started]
    try:
        code = run_process(args, child_env, wait, cwd=script.parents[2], grace_seconds=TERMINATION_GRACE_SECONDS)
    except ProcessTimeout as error:
        return 30 if error.returncode == 30 else 20
    except (subprocess.TimeoutExpired, KeyboardInterrupt):
        return 20
    if elapsed(started) > limit and code != 30:
        return 20
    if mode == "bootstrap" and code == 10:
        return 20
    return code if code in (0, 10, 20, 30, 40, 50) else 20


def overtaken(receipts):
    """Every independent member has a final at a later develop source, read without credentials."""
    if not receipts or GROUP & receipts.keys():
        return False
    try:
        later = later_final_sources(receipts)
        # A final's peeled commit is its source; its first parents must reach this source.
        return all(any(commit != receipts[key]["source"]
                       and receipts[key]["source"] in DevelopHistory.from_head(commit).positions
                       for commit in later[key]) for key in receipts)
    except (Refusal, OSError, ValueError, subprocess.SubprocessError):
        return False


def controller_receipts(receipts, transition, script, directory, started, env, child_env):
    def current_subset(candidates):
        return eligible_receipts(candidates, require_current_versions,
                                 lambda: ContractPolicy(DevelopHistory.from_head(env["GITHUB_SHA"])),
                                 transition=transition)

    try:
        current = current_subset(receipts)
    except Refusal as error:
        # Overtaken after verification: hand an independent set to the newer
        # finals' runs before any controller read or mutation. Anything else fails.
        if str(error) == SUPERSEDED and not transition and overtaken(receipts):
            return {}, SUPERSEDED_BY_NEWER_FINAL
        raise
    record_withheld(env, receipts, current)
    if not transition or not GROUP & current.keys():
        return current, 0
    require(GROUP <= current.keys(), "Deployment policy split the contract group")

    try:
        configured = configured_group(script, directory, current, started, child_env)
    except (OSError, ValueError, subprocess.SubprocessError):
        # This observation covers only AM and its runtime defaults. A sibling
        # can still proceed if its own authority is readable after the probe.
        if env.get("GITHUB_STEP_SUMMARY"):
            with open(env["GITHUB_STEP_SUMMARY"], "a", encoding="utf-8") as summary:
                summary.write("Withheld after contract configuration read failure: " + ", ".join(sorted(GROUP)) + "\n")
        independent = {key: receipt for key, receipt in current.items() if key not in GROUP}
        if independent:
            qualified = current_subset(independent)
            record_withheld(env, independent, qualified)
            return qualified, CONTRACT_READ_FAILED
        return {}, CONTRACT_READ_FAILED

    qualified = current_subset(current)
    record_withheld(env, current, qualified)
    if configured == GROUP and GROUP <= qualified.keys():
        # Matching defaults do not establish which template is serving.
        return {key: receipt for key, receipt in qualified.items() if key not in GROUP}, UNVERIFIED_COHORT
    # A partial match needs the complete current group in one controller request.
    return qualified, 0


def deploy(env, masker, script_reader=private_script, runner=execute,
           tool_reader=tool_environment, policy_reader=deploy_selection, on_disabled=lambda: None,
           on_private_refusal=None, on_unverified=None, on_rolled_back=None, on_overtaken=lambda _keys: None):
    event_identity(env)
    value = validate_set(document(env.get("CD_RELEASE_SET")))
    mode, started = env.get("CD_DEPLOY_MODE"), env.get("CD_STARTED_AT")
    automatic = env["GITHUB_EVENT_NAME"] == "push"
    with precondition("mode", refusal_code=40):
        require(mode in ("release", "bootstrap") and (not automatic or mode == "release"),
                "Deployment mode is invalid")
        require(mode != "bootstrap" or set(value["receipts"]) == {"am", "eg", "web"},
                "Bootstrap requires all components")
    limit = controller_seconds(mode)
    credentials = {key: env.get(key) for key in ("CD_NYXID_CLIENT_ID", "CD_NYXID_CLIENT_SECRET")}
    for credential in credentials.values():
        masker(credential)
    with precondition("budget"):
        remaining(started, mode=mode)
    with precondition("checkout"):
        script = script_reader(env)
    with precondition("policy"):
        selected = policy_reader(script, value["receipts"], automatic=automatic, mode=mode)
    if set(selected) != set(value["receipts"]):
        on_disabled()
    if not selected:
        return DISABLED
    with precondition("credentials-format"):
        require(all(isinstance(value, str) and bool(value) and not any(c.isspace() for c in value)
                    for value in credentials.values()),
                "Deployment credentials are missing or malformed")
    child_env = {key: env[key] for key in (*IDENTITY_KEYS, *PROCESS_KEYS) if key in env}
    child_env.update(tool_reader(env))
    child_env.update(credentials)
    if on_private_refusal is not None or on_unverified is not None or on_rolled_back is not None:
        child_env["CD_REPORT_REFUSAL"] = "1"
    # A set has independent source SHAs; the file is the sole source authority.
    # App tokens, registry credentials, keys and Actions command files stay out.
    receipts = {key: value["receipts"][key] for key in selected}
    result = None
    private_reason = None
    group_outcome = 0
    verification = None
    rollback = None
    with tempfile.TemporaryDirectory(prefix="cma-release-set-", dir=env["RUNNER_TEMP"]) as directory:
        path = Path(directory) / "release-set.json"
        with precondition("authority"):
            if automatic:
                selected_keys = sorted(receipts)
                receipts, group_outcome = controller_receipts(
                    receipts, value.get("contract_transition"), script, directory, started, env, child_env)
                if group_outcome == SUPERSEDED_BY_NEWER_FINAL:
                    on_overtaken(selected_keys)
                if not receipts:
                    return group_outcome
            request = private_document(release_set(receipts))
            with path.open("x", encoding="utf-8") as stream:
                os.chmod(path, 0o600)
                stream.write(canonical(request))
        with precondition("budget"):
            remaining(started, mode=mode)
        try:
            code = runner(script, path, mode, started, child_env)
            if code == 20 and on_unverified is not None:
                verification = read_verification(path.with_name("verification-code"), request)
            if code == 10 and on_rolled_back is not None:
                # Newer controllers bind the forward failure to the same packet.
                # Older ones write none; the recovery token is never used here.
                rollback = read_verification(path.with_name("verification-code"), request)
            if code != 30 and elapsed(started) > limit:
                # execute() also maps a late successful child to 20. Its code-0
                # result cannot certify success and is not a malformed result.
                if on_unverified is not None:
                    on_unverified(*(verification or ("phase-budget", "unreported")))
                return 20
            try:
                result = read_component_result(path.with_name("component-result.json"), request, code)
            except (OSError, ValueError, TypeError, KeyError):
                if on_unverified is not None:
                    on_unverified("result", "unreported")
                return 20
            if code == 30 and on_private_refusal is not None:
                private_reason = read_refusal(path.with_name("refusal-code"), request)
        except Exception:  # noqa: BLE001 - a launched controller may have mutated.
            code = 20
    if elapsed(started) > limit and code != 30:
        if on_unverified is not None:
            on_unverified(*(verification or ("phase-budget", "unreported")))
        return 20
    if result is not None:
        try:
            retain_component_result(result, env)
            from staging_cd_deploy_result import emit
            emit(result, request, code, env)
        except (OSError, ValueError, KeyError, TypeError):
            if on_unverified is not None:
                on_unverified("result", "unreported")
            return 20
    if verification is not None:
        on_unverified(*verification)
    if private_reason is not None:
        on_private_refusal(private_reason)
    if code == 10 and on_rolled_back is not None:
        on_rolled_back(rollback[0] if rollback is not None else "unreported")
    if code == 0:
        return group_outcome
    return code if code in (10, 20, 30, 40, 50) else 20


def explain(outcome, reason, private, verification, recovery, release, site, raised):
    """Report an exit 10, 20, 30, 40 or 52 outcome with its cause and next step.

    No component is named: the controller's packets do not say which member of
    the release set failed, and naming the whole set would blame the others.
    """
    if outcome == 20:
        return report("deploy-verification", verification, subject="deploy", recovery=recovery)
    if outcome == 10:
        # Exit 10 alone certifies the rollback; only the release token is reported.
        return report("deploy-release", release, subject="deploy")
    if private is not None:
        return report("deploy-private", private, subject="deploy")
    if reason is not None:
        return report("deploy-precondition", reason, subject="deploy", site=site)
    if outcome == 30:
        return report("deploy-private", "unreported", subject="deploy")
    if outcome == 40:
        # A failure raised here is an input refused before launch; a plain 40
        # is the controller's own input or configuration refusal.
        if raised:
            return report("deploy-precondition", "input", subject="deploy", site=site)
        return report("deploy-controller", "input-error", subject="deploy")
    if outcome == CONTRACT_READ_FAILED:
        return report("deploy-contract", "contract-read-failed", subject="deploy")
    return None


def main():
    outcome, skipped = 40, False
    reason, private, site, raised = None, None, None, False
    verification = "unreported"
    recovery = "unreported"
    release = "unreported"
    overtaken_keys = []
    try:
        entry_elapsed = elapsed(os.environ.get("CD_STARTED_AT"))
    except (ValueError, TypeError, OverflowError):
        entry_elapsed = None
    def disabled():
        nonlocal skipped
        skipped = True
    def private_refusal(token):
        nonlocal private
        if token in REPORTED_PRIVATE:
            private = token

    def unverified(first, second):
        nonlocal verification, recovery
        if first in REPORTED_VERIFICATION and second in REPORTED_VERIFICATION:
            verification, recovery = first, second

    def rolled_back(token):
        nonlocal release
        if token in REPORTED_VERIFICATION:
            release = token

    def operation(masker):
        nonlocal outcome, reason, site, raised
        try:
            outcome = deploy(os.environ, masker, on_disabled=disabled, on_private_refusal=private_refusal,
                             on_unverified=unverified, on_rolled_back=rolled_back,
                             on_overtaken=overtaken_keys.extend)
        except NotAdmitted as refusal:
            outcome = refusal.code
            # The precondition's own check, never the wrapper, names the line.
            site = error_site(refusal.__cause__) if refusal.__cause__ is not None else None
            if refusal.reason in PRECONDITIONS:
                reason = refusal.reason
        except Exception as error:
            site, raised = error_site(error), True
            raise
        return outcome in (0, DISABLED, SUPERSEDED_BY_NEWER_FINAL)
    result = run_private(operation, None, subject="deploy", explain=False)
    if result == 0 and outcome == SUPERSEDED_BY_NEWER_FINAL:
        # Nothing was deployed; fixed component tokens only cross this boundary.
        print(f"SKIP deployment reason={OVERTAKEN} components={','.join(overtaken_keys)}: "
              "a newer final's run owns this deployment")
        if os.environ.get("GITHUB_OUTPUT"):
            with open(os.environ["GITHUB_OUTPUT"], "a", encoding="utf-8") as stream:
                stream.write(f"disposition={OVERTAKEN}\n")
    diagnostic = explain(outcome, reason, private, verification, recovery, release, site, raised)
    if skipped:
        print(RESULTS[DISABLED])
    if outcome == UNVERIFIED_COHORT:
        print(RESULTS[outcome])
    if outcome == 20:
        print("::error::Staging deployment is unverified. Immediate operator intervention required.")
    summary = os.environ.get("GITHUB_STEP_SUMMARY")
    if summary:
        with open(summary, "a", encoding="utf-8") as stream:
            if skipped and outcome != DISABLED:
                stream.write(RESULTS[DISABLED] + "\n")
            if outcome == SUPERSEDED_BY_NEWER_FINAL:
                stream.write(f"Deployment skipped: {OVERTAKEN} for {', '.join(overtaken_keys)}.\n")
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
            if diagnostic:
                stream.write(diagnostic + "\n")
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
                report("deploy-component", "deployed-compatibility-unqualified", subject="deploy", component=component)
    if result == 0:
        return 0
    return outcome if outcome in (10, 20, 30, 40, 50, UNVERIFIED_COHORT, CONTRACT_READ_FAILED) else 40


if __name__ == "__main__":
    raise SystemExit(main())
