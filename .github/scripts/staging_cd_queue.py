"""Reconcile first-parent changes using the same selector as pull-request CI."""
import json
import os
import subprocess
import sys
import tempfile
import time
from pathlib import Path

from ci_tracks.git_comparison import first_parent as comparison_parent
from ci_outcome import report
from staging_cd_comparisons import SelectionIncomplete, batch_selection
from staging_cd_admission import event_identity
from staging_cd_common import matches, require
from staging_cd_compatibility import GROUP
from staging_cd_plan_window import (PlanRefused, earlier_transition, introductions, plan_window, read_finals,
                                    window_line, window_warning)
from staging_cd_resolve import output
from staging_cd_runtime import run_private
from staging_cd_history import DevelopHistory
from staging_cd_transition import transition_in_window
from staging_cd_versions import DEPLOYABLES

GROUP_IMAGES = frozenset(DEPLOYABLES[key] for key in GROUP)
# Why a plan was refused, as the release-plan outcome code its gate reports.
PLAN_REFUSALS = {"unreadable": "finals-unreadable", "denied": "finals-denied", "untrusted": "finals-untrusted",
                 "floor": "floor-required", "anchor": "anchor-unselected", "introduced": "manifest-boundary"}


def git(root, *args):
    result = subprocess.run(["git", *args], cwd=root, capture_output=True, text=True, timeout=120, check=False)
    require(result.returncode == 0, "Release history could not be reconciled")
    return result.stdout.strip()


def first_parent(root, adoption, head, command=git):
    require(all(matches(r"[0-9a-f]{40}", value) for value in (adoption, head)),
            "Release adoption and head must be immutable commits")
    rows = command(root, "rev-list", "--first-parent", "--max-count=2000", head).splitlines()
    require(adoption in rows, "Adoption commit is not on develop's first-parent history")
    # The fixed adoption merge is the first release input, inclusive. Setting
    # the variable to this workflow's merge must publish its changed images.
    pending = list(reversed(rows[:rows.index(adoption) + 1]))
    require(len(pending) <= 2000 and all(matches(r"[0-9a-f]{40}", value) for value in pending),
            "Release history exceeds the reconciliation bound")
    return pending


def selection(root, revision):
    selector = root / ".github/scripts/ci_tracks/selection.py"
    require(selector.is_file(), "The shared CI/CD selector is unavailable")
    with tempfile.TemporaryDirectory(prefix="cma-cd-selection-") as directory:
        event = Path(directory) / "event.json"
        event.write_text(json.dumps({"after": revision}), encoding="utf-8")
        child_env = {k: v for k, v in os.environ.items() if k in ("PATH", "HOME", "LANG", "LC_ALL", "RUSTUP_HOME", "CARGO_HOME")}
        result = subprocess.run([sys.executable, "-B", str(selector), "--root", str(root),
                                 "--event", str(event), "--event-name", "push"], env=child_env,
                                capture_output=True, text=True, timeout=300, check=False)
    require(result.returncode == 0 and len(result.stdout) <= 1024 * 1024,
            "Shared selector failed closed")
    value = json.loads(result.stdout)
    require(isinstance(value, dict) and value.get("head") == revision
            and value.get("base") == (comparison_parent(root, revision) or revision),
            "Shared selector comparison identity differs")
    deployables = value.get("deployables")
    require(isinstance(deployables, list) and all(isinstance(item, str) for item in deployables)
            and deployables == sorted(set(deployables)) and set(deployables).issubset(DEPLOYABLES.values()),
            "Shared selector deployables are invalid")
    return deployables


# The plan job allows 45 minutes. Stop selecting new snapshots after 35, so a
# cold selection cache still leaves time to save what was analysed.
BUDGET_SECONDS = 2100


def change_row(root, revision, decision, command=git):
    change = {"source": revision, "tree": command(root, "rev-parse", revision + "^{tree}"),
              "deployables": decision["deployables"]}
    if decision.get("selection_reasons"):
        change["selection_reasons"] = decision["selection_reasons"]
    return change


def reconcile(root, adoption, head, selector=selection, command=git, metrics=None, budget=None,
              keep_first=False):
    """Selected first-parent changes from adoption through head, in ancestry order.

    With keep_first the first revision is listed even when it selects nothing,
    so allocation can resume a lane's newest final there (staging_cd_baselines).
    """
    revisions = first_parent(root, adoption, head, command)
    selections = batch_selection(root, revisions, metrics, budget) if selector is selection else None
    results = []
    for revision in revisions:
        decision = selections[revision] if selections is not None else {"deployables": selector(root, revision)}
        if decision["deployables"] or (keep_first and revision == revisions[0]):
            results.append(change_row(root, revision, decision, command))
    return results


def release_queue(root, adoption, head, event_source, metrics, budget=None, finals=None, window=None):
    """Plan develop's release queue from the adoption commit or, when
    `finals(history)` reads each lane's newest final, from the oldest revision
    a current lane can still need (staging_cd_plan_window).

    A bounded plan does everything the adoption window does, plus resumes of a
    lane's newest final that the adoption window would strand. `window` records
    the chosen start and why. Raises PlanRefused when no start can be proved.
    """
    deadline = None if budget is None else time.monotonic() + budget
    history = DevelopHistory(head, git(root, "rev-list", "--first-parent", "--max-count=2000", head).splitlines())
    require(event_source in history.positions,
            "Push commit is no longer on develop")
    if finals is None:
        rows = reconcile(root, adoption, head, metrics=metrics, budget=budget)
        return queue_of(head, event_source, rows, transition_in_window(root, history, rows))
    window = {} if window is None else window
    require(not adoption or matches(r"[0-9a-f]{40}", adoption), "Release adoption and head must be immutable commits")

    def remaining():
        # Every batch shares one budget with the tag reads before it.
        return None if deadline is None else max(1, int(deadline - time.monotonic()))

    develop, lanes = finals(history)
    if lanes is None:
        # Allocation only plans the develop head; the newer push's run plans this one too.
        window.update(stale=develop, head=head)
        return queue_of(head, event_source, [], None)
    plan = plan_window(history, adoption, lanes, introductions(root, history))
    start = plan.start
    window.update(start=start, revisions=1 - history.position(start), adoption=adoption or "none",
                  bound_by=plan.bound_by)
    # Contract transitions older than the adoption commit stay out, as before,
    # and none before a lane's manifest addition can be compared at all.
    floor = adoption if adoption in history.positions else min(history.positions, key=history.position)
    if plan.after is not None and history.position(plan.after) > history.position(floor):
        floor = plan.after
    rows = []
    anchor = earlier_transition(root, history, floor, start)
    if anchor is not None:
        # A transition older than every lane's newest final still couples its
        # group's rollout. Select it alone and uncached, before the window, so
        # the window's cached records are never pruned to it.
        decision = batch_selection(root, [anchor], None, remaining(), cached=False)[anchor]
        if not set(decision["deployables"]) & GROUP_IMAGES:
            raise PlanRefused("anchor", GROUP, anchor)
        rows.append(change_row(root, anchor, decision))
        window["transition"] = anchor
    rows += reconcile(root, start, head, metrics=metrics, budget=remaining(), keep_first=True)
    return queue_of(head, event_source, rows,
                    transition_in_window(root, history, [row for row in rows if row["deployables"]]))


def queue_of(head, event_source, rows, transition):
    queue = {"head": head, "event_source": event_source, "changes": rows}
    if transition:
        queue["contract_transition"] = transition
    return queue


def main(argv=None):
    argv = sys.argv[1:] if argv is None else argv
    if argv == ["--require-complete"]:
        # The plan step succeeded so that its records were saved; fail the job here.
        report("release-plan", PLAN_REFUSALS.get(os.environ.get("CD_PLAN_REFUSED", ""), "incomplete"),
               subject="release-queue")
        return 1
    if argv:
        report("step", "refused", subject="release-queue")
        return 1
    env, root, metrics, progress, window = os.environ, Path.cwd(), {}, {}, {}
    def operation(_masker):
        event_identity(env)
        require(env["GITHUB_EVENT_NAME"] == "push", "Allocation planning only accepts develop pushes")
        head = git(root, "rev-parse", "HEAD")
        require(head == env["GITHUB_SHA"], "Release checkout differs from the push")
        try:
            queue = release_queue(root, env.get("CD_ADOPTION_SHA"), head, env["GITHUB_SHA"], metrics,
                                  budget=BUDGET_SECONDS, window=window,
                                  finals=lambda history: read_finals(root, env.get("GH_TOKEN"), history))
        except SelectionIncomplete as stopped:
            # Nothing is planned, so nothing downstream runs. A later step fails
            # the job after the analysed records are saved for the next run; a
            # transient stop is the one catch-up re-runs.
            output(env, "complete", "false")
            output(env, "transient", "true")
            progress.update(decided=stopped.decided, total=stopped.total)
            return True
        except PlanRefused as refused:
            # Likewise, and its gate names the refusal. The next push plans again.
            output(env, "complete", "false")
            output(env, "refused", refused.kind)
            output(env, "transient", "true" if refused.kind == "unreadable" else "false")
            window.update(refused=refused.kind, lanes=refused.lanes, at=refused.at)
            return True
        payload = json.dumps(queue, separators=(",", ":"))
        require(len(payload) < 750000, "Release queue exceeds the workflow output limit")
        output(env, "queue", payload)
        output(env, "changed", "true" if queue["changes"] else "false")
        output(env, "complete", "true")
        return True
    status = run_private(operation, None, subject="release-queue")
    if window:
        print(window_line(window), flush=True)
        warning = window_warning(window) if "refused" not in window else None
        if warning:
            print(warning, flush=True)
    if status == 0 and progress:
        print(f"INCOMPLETE release-queue decided={progress['decided']} total={progress['total']} "
              f"budget_seconds={BUDGET_SECONDS}", flush=True)
    if status == 0 and metrics:
        print(json.dumps({'event': 'selection-cache', **metrics}, sort_keys=True))
    return status


if __name__ == "__main__":
    raise SystemExit(main())
