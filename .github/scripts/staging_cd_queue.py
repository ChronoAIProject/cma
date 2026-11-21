"""Reconcile first-parent changes using the same selector as pull-request CI."""
import json
import os
import subprocess
import sys
import tempfile
from pathlib import Path

from ci_tracks.git_comparison import first_parent as comparison_parent
from staging_cd_comparisons import batch_selection
from staging_cd_admission import event_identity
from staging_cd_common import matches, require
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


def reconcile(root, adoption, head, selector=selection, command=git, metrics=None):
    results = []
    revisions = first_parent(root, adoption, head, command)
    selections = batch_selection(root, revisions, metrics) if selector is selection else None
    for revision in revisions:
        decision = selections[revision] if selections is not None else {"deployables": selector(root, revision)}
        selected = decision["deployables"]
        if selected:
            change = {"source": revision, "tree": command(root, "rev-parse", revision + "^{tree}"),
                      "deployables": selected}
            if decision.get("selection_reasons"):
                change["selection_reasons"] = decision["selection_reasons"]
            results.append(change)
    return results


def release_queue(root, adoption, head, event_source, metrics):
    history = DevelopHistory(head, git(root, "rev-list", "--first-parent", "--max-count=2000", head).splitlines())
    require(event_source in history.positions,
            "Push commit is no longer on develop")
    rows = reconcile(root, adoption, head, metrics=metrics)
    queue = {"head": head, "event_source": event_source, "changes": rows}
    transition = transition_in_window(root, history, rows)
    if transition:
        queue["contract_transition"] = transition
    return queue


def main():
    env, root, metrics = os.environ, Path.cwd(), {}
    def operation(_masker):
        event_identity(env)
        require(env["GITHUB_EVENT_NAME"] == "push", "Allocation planning only accepts develop pushes")
        head = git(root, "rev-parse", "origin/develop")
        queue = release_queue(root, env.get("CD_ADOPTION_SHA"), head, env["GITHUB_SHA"], metrics)
        payload = json.dumps(queue, separators=(",", ":"))
        require(len(payload) < 750000, "Release queue exceeds the workflow output limit")
        output(env, "queue", payload)
        output(env, "changed", "true" if queue["changes"] else "false")
        return True
    status = run_private(operation, None, subject="release-queue")
    if status == 0 and metrics:
        print(json.dumps({'event': 'selection-cache', **metrics}, sort_keys=True))
    return status


if __name__ == "__main__":
    raise SystemExit(main())
