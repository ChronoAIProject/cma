"""Run the shared comparison producer and validate its public output."""
import json
import os
from pathlib import Path
import subprocess
import sys
import tempfile

from ci_tracks.git_comparison import first_parents as comparison_parents
from ci_tracks.uncertainty import REASONS
from staging_cd_common import matches, require
from staging_cd_versions import DEPLOYABLES


# The selector's own exit status for a batch its budget stopped; it prints no decisions.
INCOMPLETE = 3
STATS = ('hits', 'misses', 'input_hits', 'input_misses', 'elapsed_ms')


class SelectionIncomplete(Exception):
    """The budget stopped selection after deciding `decided` of `total` revisions."""

    def __init__(self, decided, total):
        super().__init__(decided, total)
        self.decided, self.total = decided, total


def selector_stats(stderr, keys):
    """Bounded integers from the selector's single stats line, or None."""
    try:
        stats = json.loads(stderr)
    except (ValueError, TypeError):
        return None
    if (isinstance(stats, dict) and stats.get('event') == 'selection-cache'
            and all(type(stats.get(key)) is int and 0 <= stats[key] <= 100000000 for key in keys)):
        return {key: stats[key] for key in keys}
    return None


def batch_selection(root, revisions, metrics=None, budget=None, *, cached=True):
    """Select contiguous first-parent revisions. Uncached, a batch never prunes the shared cache."""
    selector = root / ".github/scripts/ci_tracks/batch_selection.py"
    require(selector.is_file(), "Shared batch selector is unavailable")
    require(budget is None or (type(budget) is int and 0 < budget < 2400), "Selection budget is invalid")
    with tempfile.TemporaryDirectory(prefix="cma-cd-selection-") as directory:
        batch = Path(directory) / "revisions.json"
        batch.write_text(json.dumps(revisions), encoding="utf-8")
        allowed = ("PATH", "HOME", "LANG", "LC_ALL", "RUSTUP_HOME", "CARGO_HOME") + (("CD_SELECTOR_CACHE",) if cached else ())
        child_env = {k: v for k, v in os.environ.items() if k in allowed}
        command = [sys.executable, "-B", str(selector), str(root), str(batch)]
        # The budget stops new snapshots; this timeout only backs it up.
        result = subprocess.run(command + ([str(budget)] if budget is not None else []),
                                env=child_env, capture_output=True, text=True,
                                timeout=2400, check=False)
    if budget is not None and result.returncode == INCOMPLETE and not result.stdout:
        progress = selector_stats(result.stderr, ('decided', 'total'))
        require(progress is not None and progress['decided'] < progress['total'] == len(revisions),
                "Shared batch selector progress is invalid")
        if metrics is not None:
            metrics.update(selector_stats(result.stderr, STATS) or {})
        raise SelectionIncomplete(progress['decided'], progress['total'])
    require(result.returncode == 0 and len(result.stdout) <= 8 * 1024 * 1024,
            "Shared batch selector failed closed")
    values = json.loads(result.stdout)
    require(isinstance(values, list) and len(values) == len(revisions), "Selector comparison count differs")
    parents = comparison_parents(root, revisions)
    selections = {}
    for revision, value in zip(revisions, values):
        require(isinstance(value, dict) and value.get("head") == revision
                and value.get("base") == (parents[revision] or revision),
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
        # Observability never changes admission or echoes child output.
        metrics.update(selector_stats(result.stderr, STATS) or {})
    return selections
