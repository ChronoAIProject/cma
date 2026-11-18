"""Reconcile first-parent changes using the same selector as pull-request CI."""
import json
import os
import subprocess
import sys
import tempfile
from pathlib import Path

from staging_cd_admission import event_identity
from staging_cd_common import matches, require
from staging_cd_github import GitHub
from staging_cd_resolve import output
from staging_cd_runtime import run_private
from staging_cd_versions import DEPLOYABLES


def git(root, *args):
    result = subprocess.run(["git", *args], cwd=root, capture_output=True, text=True, timeout=120, check=False)
    require(result.returncode == 0, "Release history could not be reconciled")
    return result.stdout.strip()


def first_parent(root, adoption, head, command=git):
    require(all(matches(r"[0-9a-f]{40}", value) for value in (adoption, head)),
            "Release adoption and head must be immutable commits")
    rows = command(root, "rev-list", "--first-parent", head).splitlines()
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
            and value.get("base") == git(root, "rev-parse", revision + "^1"),
            "Shared selector comparison identity differs")
    deployables = value.get("deployables")
    require(isinstance(deployables, list) and all(isinstance(item, str) for item in deployables)
            and deployables == sorted(set(deployables)) and set(deployables).issubset(DEPLOYABLES.values()),
            "Shared selector deployables are invalid")
    return deployables


def reconcile(root, adoption, head, selector=selection, command=git):
    results = []
    for revision in first_parent(root, adoption, head, command):
        selected = selector(root, revision)
        if selected:
            results.append({"source": revision, "tree": command(root, "rev-parse", revision + "^{tree}"),
                            "deployables": selected})
    return results


def plan_queue(root, adoption, event_source, *, command=git, selector=selection):
    # Checkout can reset origin/develop to the event on a rerun. Fetch the
    # actual branch without changing this job's reviewed workflow checkout.
    command(root, "fetch", "--no-tags", "origin", "refs/heads/develop")
    head = command(root, "rev-parse", "FETCH_HEAD^{commit}")
    require(matches(r"[0-9a-f]{40}", head) and matches(r"[0-9a-f]{40}", event_source),
            "Fetched develop or push identity is invalid")
    require(event_source in command(root, "rev-list", "--first-parent", head).splitlines(),
            "Push commit is no longer on develop")
    rows = reconcile(root, adoption, head, selector, command)
    return {"head": head, "event_source": event_source, "changes": rows}


def main():
    env, root = os.environ, Path.cwd()
    def operation(_masker):
        event_identity(env)
        require(env["GITHUB_EVENT_NAME"] == "push", "Allocation planning only accepts develop pushes")
        queue = plan_queue(root, env.get("CD_ADOPTION_SHA"), env["GITHUB_SHA"])
        queue["workflow_source"] = git(root, "rev-parse", "HEAD")
        require(queue["workflow_source"] == env["GITHUB_SHA"], "Planning workflow checkout differs from its event")
        from staging_cd_retirements import load_policy, resolve_retirements
        retirements = resolve_retirements(load_policy(root), queue, GitHub(env.get("GH_TOKEN")), root, selector=selection)
        queue["dispositions"] = retirements.dispositions
        payload = json.dumps(queue, separators=(",", ":"))
        require(len(payload) < 750000, "Release queue exceeds the workflow output limit")
        output(env, "queue", payload)
        output(env, "head", queue["head"])
        output(env, "changed", "true" if queue["changes"] else "false")
        evidence = Path(env["CD_QUEUE_PATH"])
        evidence.write_text(payload + "\n", encoding="utf-8")
        return True
    return run_private(operation, None)


if __name__ == "__main__":
    raise SystemExit(main())
