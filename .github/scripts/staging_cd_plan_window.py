"""Start release reconciliation at the oldest revision a current lane can still need.

A lane's final release tag records the develop source it was built from, and
that build used the whole tree at that source. staging_cd_allocate acts for a
lane only at the lane's newest listed source, and skips even that one when any
reservation or final of the lane has a later source. So no first-parent
revision before a lane's newest final can change what allocation does for that
lane, and reconciliation can start at the oldest newest-final source over all
current lanes. That source stays in the window: a final whose source no longer
selects its lane is resumed only from a listed source (staging_cd_baselines).

The start is never older than CMA_CD_ADOPTION_SHA. A lane without a newest
final that is verified and on develop first-parent history at or after it keeps
the adoption commit as its start, so anything unverifiable only widens the
window back to the adoption commit, never past a revision a lane still needs.
The adoption window also keeps the newest AM/runtime contract transition, which
couples its group's rollout even after every lane has a newer final; a bounded
plan lists that one merge as well (earlier_transition).
"""
import subprocess
import time

from staging_cd_catch_up import advertised, final_source, lane_versions
from staging_cd_common import Refusal
from staging_cd_compatibility import ContractPolicy, RECIPE, VERSION_FILE
from staging_cd_github import GitHub
from staging_cd_receipts import final_tag_oid, parse_annotation
from staging_cd_release_tags import local_tag_reader
from staging_cd_versions import DEPLOYABLES, version_tuple

# Five tag objects normally take seconds to read. A slow read only widens the window.
FINAL_SECONDS = 60
UNREADABLE = (OSError, ValueError, KeyError, TypeError, subprocess.SubprocessError)


def newest_finals(api, refs):
    """Each current lane's newest final source, or None and why it cannot bound the window.

    Refs come from one live advertisement. Each tag object is read through the
    same reader and checks as catch-up, and must also be byte for byte the
    canonical tag its own receipt produces, so its ID authenticates it.
    """
    try:
        lanes = lane_versions(refs)
    except Refusal:
        return dict.fromkeys(DEPLOYABLES, (None, "malformed"))
    result = {}
    for component, (finals, _pending) in sorted(lanes.items()):
        if not finals:
            result[component] = (None, "no-final")
            continue
        version = max(finals, key=version_tuple)
        oid = finals[version]
        try:
            source = final_source(api, component, version, oid)
            # The reader keeps the verified object, so this parses the same bytes.
            receipt = parse_annotation(api.request("git/tags/" + oid).get("message"))
        except UNREADABLE:
            result[component] = (None, "unverified")
            continue
        result[component] = (source, "final") if final_tag_oid(receipt) == oid else (None, "unverified")
    return result


def read_finals(root, token, *, runner=subprocess.run, seconds=FINAL_SECONDS):
    """Newest finals from the public tag advertisement; unreadable lanes widen the window."""
    deadline = time.monotonic() + seconds
    try:
        refs = advertised(runner)
        api = GitHub(token, runner, local_tags=local_tag_reader(root, deadline=deadline), deadline=deadline)
    except UNREADABLE:
        return dict.fromkeys(DEPLOYABLES, (None, "unreadable"))
    return newest_finals(api, refs)


def plan_window(history, adoption, finals):
    """Return the window's first revision and the lanes that put it there.

    Every current lane bounds the start at its newest final's source, or at
    the adoption commit when that final is missing, unverified, off develop's
    first-parent history or older than the adoption commit. The start is the
    oldest bound, so each lane still sees every revision from its own bound.
    """
    if adoption not in history.positions:
        return adoption, []  # Reconciliation refuses it with its own message.
    floor = history.positions[adoption]
    bounds = {}
    for component in sorted(DEPLOYABLES):
        source, reason = finals.get(component, (None, "unreadable"))
        if source is None:
            bounds[component] = floor, reason
        elif source not in history.positions:
            bounds[component] = floor, "off-history"
        elif history.positions[source] < floor:
            bounds[component] = floor, "before-adoption"
        else:
            bounds[component] = history.positions[source], "final"
    start = min(position for position, _reason in bounds.values())
    commit = next(sha for sha, position in history.positions.items() if position == start)
    return commit, [component if reason == "final" else component + ":" + reason
                    for component, (position, reason) in bounds.items() if position == start]


def earlier_transition(root, history, adoption, start):
    """The newest AM/runtime contract transition at or after adoption and before start.

    The adoption window keeps such a transition, and with it the rollout of
    its whole group, after every lane has a newer final. Its search reads only
    merges that touched a contract input, so finding it costs no selection.
    """
    policy = ContractPolicy(history, root=root)
    commits = policy.git("log", "--first-parent", "--max-count=2000", "--format=%H", history.head,
                         "--", VERSION_FILE, RECIPE).splitlines()
    for source in commits:
        if source not in history.positions or history.position(source) < history.position(adoption):
            break
        if history.position(source) < history.position(start) and policy.transition(source):
            return source
    return None


def window_line(window):
    line = (f"PLAN window start={window['start']} revisions={window['revisions']} "
            f"adoption={window['adoption']} bound-by={','.join(window['bound_by'])}")
    return line + (f" transition={window['transition']}" if window.get("transition") else "")
