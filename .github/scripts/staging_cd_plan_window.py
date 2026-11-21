"""Start release reconciliation at the oldest revision a current lane can still need.

A lane's final release tag records the develop source it was built from, and
that build used the whole tree at that source. staging_cd_allocate acts for a
lane only at the lane's newest listed source, and skips even that one when any
reservation or final of the lane has a later source. So no first-parent
revision before a lane's newest final can change what allocation does for that
lane. A lane that never had a final is selected by the merge that added it to
the deployable manifest, so nothing before that merge can be its newest
selected source; the plan refuses if the window does not select it.

The window starts at the oldest of these lane bounds, so every lane still sees
every revision from its own bound, and its first revision is listed even when
it selects nothing. That listing lets a final whose source no longer selects
its lane resume (staging_cd_baselines); the adoption window lists only selected
revisions and strands such a final. So a bounded plan does everything the
adoption window does, plus resumes the adoption window would strand.

CMA_CD_ADOPTION_SHA is no longer required. When it is on this plan's history it
is still a lower bound, and contract transitions older than it stay out of the
plan. It is required only for a lane whose bound (its newest final, or for a
new lane the merge that added it) is not on the last 2,000 first-parent
merges: that plan refuses at once, naming the lane, until the floor is set.
A tag that cannot be read or verified also refuses the plan at once, so the
next push plans again; nothing is skipped, because nothing was planned. A run
whose push is no longer the develop head plans nothing: allocation would find
it stale, and the newer push's run plans its merges.
"""
import json
import subprocess
import time

from staging_cd_catch_up import advertised, final_source, lane_versions
from staging_cd_common import Refusal
from staging_cd_compatibility import ContractPolicy, RECIPE, VERSION_FILE
from staging_cd_github import GitHub
from staging_cd_receipts import final_tag_oid, parse_annotation
from staging_cd_release_tags import local_tag_reader
from staging_cd_versions import DEPLOYABLES, version_tuple

# Five tag objects normally take seconds to read; the plan refuses after this.
FINAL_SECONDS = 60
MANIFEST = ".github/scripts/deployables.json"
UNREADABLE = (OSError, ValueError, KeyError, TypeError, subprocess.SubprocessError)
# Refusals, most actionable first.
KINDS = ("untrusted", "floor", "unreadable")


class PlanRefused(Exception):
    """No window can be proved now. `kind` names why, `lanes` which lanes."""

    def __init__(self, kind, lanes):
        super().__init__(kind)
        self.kind, self.lanes = kind, sorted(lanes)


def newest_finals(api, refs):
    """Each current lane's newest final source and kind: final, new, unreadable or untrusted.

    Refs come from one live advertisement. Each tag object is read through the
    same reader and checks as catch-up, and must also be byte for byte the
    canonical tag its own receipt produces, so its ID authenticates it.
    """
    try:
        lanes = lane_versions(refs)
    except Refusal:
        return dict.fromkeys(DEPLOYABLES, (None, "untrusted"))
    result = {}
    for component, (finals, _pending) in sorted(lanes.items()):
        if not finals:
            result[component] = (None, "new")
            continue
        version = max(finals, key=version_tuple)
        oid = finals[version]
        try:
            api.request("git/tags/" + oid)  # A failed read is no verdict on the tag.
        except UNREADABLE:
            result[component] = (None, "unreadable")
            continue
        try:
            # The reader keeps the object it read, so these checks see the same bytes.
            source = final_source(api, component, version, oid)
            receipt = parse_annotation(api.request("git/tags/" + oid).get("message"))
        except UNREADABLE:
            result[component] = (None, "untrusted")
            continue
        result[component] = (source, "final") if final_tag_oid(receipt) == oid else (None, "untrusted")
    return result


def read_finals(root, token, history, *, runner=subprocess.run, seconds=FINAL_SECONDS):
    """Return the advertised develop head and each lane's newest final.

    A head that is neither this plan's head nor one of its ancestors is newer:
    allocation will find this run stale and do nothing, so finals are not read.
    """
    deadline = time.monotonic() + seconds
    try:
        refs = advertised(runner)
        if refs["refs/heads/develop"] not in history.positions:
            return refs["refs/heads/develop"], None
        api = GitHub(token, runner, local_tags=local_tag_reader(root, deadline=deadline), deadline=deadline)
    except UNREADABLE:
        return None, dict.fromkeys(DEPLOYABLES, (None, "unreadable"))
    return refs["refs/heads/develop"], newest_finals(api, refs)


def introduction(root, history, name):
    """The newest first-parent merge that added `name` to the deployable manifest, or None."""
    policy = ContractPolicy(history, root=root)

    def names(commit):
        if not policy.git("ls-tree", "--name-only", commit, "--", MANIFEST).strip():
            return set()
        return set(json.loads(policy.git("show", commit + ":" + MANIFEST)))

    for commit in policy.git("log", "--first-parent", "--max-count=2000", "--format=%H", history.head,
                             "--", MANIFEST).splitlines():
        if commit not in history.positions:
            break
        header = policy.git("cat-file", "commit", commit).split("\n\n", 1)[0]
        parents = [line[7:] for line in header.splitlines() if line.startswith("parent ")]
        if name in names(commit) and not (parents and name in names(parents[0])):
            return commit
    return None


def plan_window(history, adoption, finals, introduced):
    """Return the window's first revision, the lanes that put it there, and the
    manifest introductions of new lanes that the window relies on.

    `introduced(name)` finds a new lane's introduction. Raises PlanRefused when a
    lane's bound cannot be proved.
    """
    floor = adoption if adoption in history.positions else None
    bounds, relied, refused = {}, {}, {}
    for component in sorted(DEPLOYABLES):
        source, kind = finals.get(component, (None, "unreadable"))
        if kind in ("unreadable", "untrusted"):
            refused.setdefault(kind, []).append(component)
            continue
        if kind == "new":
            source = introduced(DEPLOYABLES[component])
        if source in history.positions:
            bounds[component] = history.positions[source], kind
            if kind == "new":
                relied[component] = source
        elif floor is not None:
            bounds[component] = history.positions[floor], "floor"
        else:
            refused.setdefault("floor", []).append(component)
    for kind in KINDS:
        if kind in refused:
            raise PlanRefused(kind, refused[kind])
    start = min(position for position, _kind in bounds.values())
    lanes = [component if kind == "final" else component + ":" + kind
             for component, (position, kind) in bounds.items() if position == start]
    if floor is not None and history.positions[floor] > start:
        start, lanes = history.positions[floor], ["adoption"]
    commit = next(sha for sha, position in history.positions.items() if position == start)
    return commit, lanes, relied


def earlier_transition(root, history, floor, start):
    """The newest AM/runtime contract transition at or after floor and before start.

    The adoption window keeps such a transition, and with it the rollout of
    its whole group, after every lane has a newer final. Its search reads only
    merges that touched a contract input, so finding it costs no selection.
    """
    policy = ContractPolicy(history, root=root)
    commits = policy.git("log", "--first-parent", "--max-count=2000", "--format=%H", history.head,
                         "--", VERSION_FILE, RECIPE).splitlines()
    for source in commits:
        if source not in history.positions or history.position(source) < history.position(floor):
            break
        if history.position(source) < history.position(start) and policy.transition(source):
            return source
    return None


def window_line(window):
    if "stale" in window:
        return f"PLAN stale develop={window['stale']} head={window['head']}"
    if "refused" in window:
        return f"PLAN refused reason={window['refused']} lanes={','.join(window['lanes'])}"
    line = (f"PLAN window start={window['start']} revisions={window['revisions']} "
            f"adoption={window['adoption']} bound-by={','.join(window['bound_by'])}")
    return line + (f" transition={window['transition']}" if window.get("transition") else "")
