"""Reconcile first-parent changes using the same selector as pull-request CI."""
import json
import os
import subprocess
import sys
import tempfile
from pathlib import Path

from staging_cd_admission import event_identity
from staging_cd_common import matches, require
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
    pending = list(reversed(rows[:rows.index(adoption)]))
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


def main():
    env, root = os.environ, Path.cwd()
    def operation(_masker):
        event_identity(env)
        require(env["GITHUB_EVENT_NAME"] == "push", "Allocation planning only accepts develop pushes")
        head = git(root, "rev-parse", "origin/develop")
        require(env["GITHUB_SHA"] in git(root, "rev-list", "--first-parent", head).splitlines(),
                "Push commit is no longer on develop")
        rows = reconcile(root, env.get("CD_ADOPTION_SHA"), head)
        payload = json.dumps({"head": head, "event_source": env["GITHUB_SHA"], "changes": rows}, separators=(",", ":"))
        require(len(payload) < 750000, "Release queue exceeds the workflow output limit")
        output(env, "queue", payload)
        output(env, "changed", "true" if rows else "false")
        return True
    return run_private(operation, None)


if __name__ == "__main__":
    raise SystemExit(main())
