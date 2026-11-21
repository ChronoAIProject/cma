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
from staging_cd_plan_window import UNREADABLE, earlier_transition, plan_window, read_finals, window_line
from staging_cd_resolve import output
from staging_cd_runtime import run_private
from staging_cd_history import DevelopHistory
from staging_cd_transition import transition_in_window
from staging_cd_versions import DEPLOYABLES


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


def reconcile(root, adoption, head, selector=selection, command=git, metrics=None, budget=None,
              keep_first=False):
    """Selected first-parent changes from adoption through head, in ancestry order.

    With keep_first the first revision is listed even when it selects nothing:
    a bounded window starts at a lane's newest final, and allocation resumes a
    final only from a listed source.
    """
    results = []
    revisions = first_parent(root, adoption, head, command)
    selections = batch_selection(root, revisions, metrics, budget) if selector is selection else None
    for revision in revisions:
        decision = selections[revision] if selections is not None else {"deployables": selector(root, revision)}
        selected = decision["deployables"]
        if selected or (keep_first and revision == revisions[0]):
            change = {"source": revision, "tree": command(root, "rev-parse", revision + "^{tree}"),
                      "deployables": selected}
            if decision.get("selection_reasons"):
                change["selection_reasons"] = decision["selection_reasons"]
            results.append(change)
    return results


def release_queue(root, adoption, head, event_source, metrics, budget=None, finals=None, window=None,
                  anchor_selector=selection):
    """Plan develop's release queue from the adoption commit, or from the oldest
    revision a current lane can still need when `finals()` reads lane finals.

    A bounded plan allocates as the adoption window would; staging_cd_plan_window
    gives the argument. `window` receives the chosen start and why.
    """
    started = time.monotonic()
    history = DevelopHistory(head, git(root, "rev-list", "--first-parent", "--max-count=2000", head).splitlines())
    require(event_source in history.positions,
            "Push commit is no longer on develop")
    window = {} if window is None else window
    start, bound = (adoption, []) if finals is None else plan_window(history, adoption, finals())
    if bound:
        window.update(start=start, revisions=1 - history.position(start), adoption=adoption, bound_by=bound)

    def adoption_window(reason):
        # Never guess which transition the adoption window would keep.
        window.update(start=adoption, revisions=1 - history.position(adoption), bound_by=[reason])
        remaining = None if budget is None else max(1, int(budget - (time.monotonic() - started)))
        return release_queue(root, adoption, head, event_source, metrics, remaining)

    rows = reconcile(root, start, head, metrics=metrics, budget=budget, keep_first=start != adoption)
    selected = [row for row in rows if row["deployables"]]
    transition = transition_in_window(root, history, selected)
    if transition is None and start != adoption:
        # A transition older than every lane's newest final still couples its
        # group's rollout in the adoption window. Keep that one merge, selected
        # alone: a separate cached batch would prune the window's records.
        try:
            anchor = earlier_transition(root, history, adoption, start)
        except UNREADABLE:
            return adoption_window("transition:unreadable")
        if anchor is not None:
            row = {"source": anchor, "tree": git(root, "rev-parse", anchor + "^{tree}"),
                   "deployables": anchor_selector(root, anchor)}
            if not set(row["deployables"]) & {DEPLOYABLES[key] for key in GROUP}:
                return adoption_window("transition:unselected")
            rows.insert(0, row)
            selected.insert(0, row)
            transition = transition_in_window(root, history, selected)
            window["transition"] = anchor
    queue = {"head": head, "event_source": event_source, "changes": rows}
    if transition:
        queue["contract_transition"] = transition
    return queue


def main(argv=None):
    argv = sys.argv[1:] if argv is None else argv
    if argv == ["--require-complete"]:
        # The plan step succeeded so that its records were saved; fail the job here.
        report("release-plan", "incomplete", subject="release-queue")
        return 1
    if argv:
        report("step", "refused", subject="release-queue")
        return 1
    env, root, metrics, progress, window = os.environ, Path.cwd(), {}, {}, {}
    def operation(_masker):
        event_identity(env)
        require(env["GITHUB_EVENT_NAME"] == "push", "Allocation planning only accepts develop pushes")
        head = git(root, "rev-parse", "origin/develop")
        try:
            queue = release_queue(root, env.get("CD_ADOPTION_SHA"), head, env["GITHUB_SHA"], metrics,
                                  budget=BUDGET_SECONDS, finals=lambda: read_finals(root, env.get("GH_TOKEN")),
                                  window=window)
        except SelectionIncomplete as stopped:
            # Nothing is planned, so nothing downstream runs. A later step fails
            # the job after the analysed records are saved for the next run.
            output(env, "complete", "false")
            progress.update(decided=stopped.decided, total=stopped.total)
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
    if status == 0 and progress:
        print(f"INCOMPLETE release-queue decided={progress['decided']} total={progress['total']} "
              f"budget_seconds={BUDGET_SECONDS}", flush=True)
    if status == 0 and metrics:
        print(json.dumps({'event': 'selection-cache', **metrics}, sort_keys=True))
    return status


if __name__ == "__main__":
    raise SystemExit(main())
