"""Run the shared comparison producer and validate its public output."""
import json
import os
from pathlib import Path
import subprocess
import sys
import tempfile

from ci_tracks.git_comparison import first_parent as comparison_parent
from ci_tracks.uncertainty import REASONS
from staging_cd_common import matches, require
from staging_cd_versions import DEPLOYABLES


def batch_selection(root, revisions, metrics=None):
    selector = root / ".github/scripts/ci_tracks/batch_selection.py"
    require(selector.is_file(), "Shared batch selector is unavailable")
    with tempfile.TemporaryDirectory(prefix="cma-cd-selection-") as directory:
        batch = Path(directory) / "revisions.json"
        batch.write_text(json.dumps(revisions), encoding="utf-8")
        child_env = {k: v for k, v in os.environ.items() if k in (
            "PATH", "HOME", "LANG", "LC_ALL", "RUSTUP_HOME", "CARGO_HOME", "CD_SELECTOR_CACHE")}
        result = subprocess.run([sys.executable, "-B", str(selector), str(root), str(batch)],
                                env=child_env, capture_output=True, text=True,
                                timeout=2400, check=False)
    require(result.returncode == 0 and len(result.stdout) <= 8 * 1024 * 1024,
            "Shared batch selector failed closed")
    values = json.loads(result.stdout)
    require(isinstance(values, list) and len(values) == len(revisions), "Selector comparison count differs")
    selections = {}
    for revision, value in zip(revisions, values):
        require(isinstance(value, dict) and value.get("head") == revision
                and value.get("base") == (comparison_parent(root, revision) or revision),
                "Selector comparison identity differs")
        deployables = value.get("deployables")
        require(isinstance(deployables, list) and all(isinstance(item, str) for item in deployables)
                and deployables == sorted(set(deployables)) and set(deployables).issubset(DEPLOYABLES.values()),
                "Selector comparison deployables are invalid")
        reasons = value.get("reasons")
        require(isinstance(reasons, list) and all(isinstance(reason, str)
                and matches(r"[a-z0-9-]+", reason) for reason in reasons)
                and reasons == sorted(set(reasons)), "Selector reasons are invalid")
        fallback = sorted(set(reasons) & REASONS)
        require(not fallback or deployables == sorted(DEPLOYABLES.values()),
                "Uncertain selection must include every deployable")
        selections[revision] = {"deployables": deployables, "selection_reasons": fallback}
    if metrics is not None:
        try:
            stats = json.loads(result.stderr)
            keys = ('hits', 'misses', 'input_hits', 'input_misses', 'elapsed_ms')
            if (isinstance(stats, dict) and stats.get('event') == 'selection-cache'
                    and all(type(stats.get(key)) is int and 0 <= stats[key] <= 100000000 for key in keys)):
                metrics.update({key: stats[key] for key in keys})
        except (ValueError, TypeError):
            pass  # Observability never changes admission or echoes child output.
    return selections
