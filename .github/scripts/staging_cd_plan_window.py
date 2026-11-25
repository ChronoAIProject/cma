"""Start release reconciliation at the oldest revision a current lane can still need.

A lane's final release tag records the develop source it was built from, and
that build used the whole tree at that source. staging_cd_allocate acts for a
lane only at the lane's newest listed source, and skips even that one when any
reservation or final of the lane has a later source. So no first-parent
revision before a lane's newest final can change what allocation does for that
lane. The window starts at the oldest such bound over all current lanes, so
every lane still sees every revision from its own bound, and its first
revision is listed even when it selects nothing. That listing lets a final
whose source no longer selects its lane resume (staging_cd_baselines); the
adoption window lists only selected revisions and strands such a final. So a
bounded plan does everything the adoption window does, plus resumes the
adoption window would strand.

The shared selector cannot compare across a merge that added a lane to the
deployable manifest: the parent's manifest lacks the lane, so it refuses. A
lane that never had a final is therefore bounded by the first merge after its
addition, and a window that would reach any current lane's addition refuses
at once (manifest-boundary): an operator sets CMA_CD_ADOPTION_SHA to a merge
after it, exactly as the adoption window always needed.

CMA_CD_ADOPTION_SHA is otherwise not required. On this plan's history it is a
lower bound, and contract transitions older than it stay out of the plan. It is
needed only for a lane whose newest final is not on the last 2,000 first-parent
merges, and that plan refuses at once, naming the lane, until it is set.

Tag reads repeat failures that may pass, honour Retry-After and give a
just-advertised object a short while to propagate. A read that still fails
stops the plan as transient: its gate is the one catch-up re-runs, once, and
the next push plans again; nothing is planned, so nothing is skipped. A run
whose push is no longer the develop head plans nothing: allocation would find
it stale, and the newer push's run plans it.
"""
from dataclasses import dataclass
import json
import subprocess
import time

import ci_retry
from staging_cd_catch_up import advertised, final_receipt, lane_versions
from staging_cd_common import Refusal, require
from staging_cd_compatibility import ContractPolicy, RECIPE, VERSION_FILE
from staging_cd_github import GitHub
from staging_cd_history import DevelopHistory
from staging_cd_release_tags import local_tag_reader
from staging_cd_versions import DEPLOYABLES, version_tuple

# Reading the tag inventory and five tag objects normally takes seconds. Slow
# reads repeat inside this budget, which the plan's selection budget includes.
FINAL_SECONDS = 240
READ_POLICY = {"attempts": 6, "base": 2.0, "cap": 30.0}
# A just-advertised tag object may 404 this long; after it a 404 is a real one.
PROPAGATION_SECONDS = 30
# The anonymous tag listing of this public repository refuses no credential:
# only these say what it returned is wrong.
MALFORMED = frozenset({"Release tag inventory is malformed", "Develop head could not be verified"})
MANIFEST = ".github/scripts/deployables.json"
UNREADABLE = (OSError, ValueError, KeyError, TypeError, subprocess.SubprocessError)
# A window this long is analysed for minutes on a cold cache; say why it is.
WIDE_REVISIONS = 150
HISTORY_LIMIT = 2000


class PlanRefused(Exception):
    """No window can be proved now. `kind` names why, `lanes` which lanes, `at` where."""

    def __init__(self, kind, lanes, at=None):
        super().__init__(kind)
        self.kind, self.lanes, self.at = kind, sorted(lanes), at


@dataclass(frozen=True)
class Window:
    start: str  # The first revision to reconcile.
    bound_by: list  # The lanes, or "adoption", that put it there.
    after: str | None  # The newest current lane's manifest addition on this history.


def object_reader(clock):
    """Classify one tag object's failed reads, and name what a final failure means.

    Only a 401 refuses the credential. A 403 can be a secondary rate limit
    without its headers, so it is repeated like any outage. A 404 is repeated
    for a short propagation window, and only a 404 that outlasted the whole
    window means the advertised final is not a tag object GitHub serves
    (deleted, or lightweight). Reads that end sooner, when attempts or the
    budget run out, are unreadable, like anything else that does not pass.
    """
    first = {}

    def classify(error):
        status = getattr(error, "status", None)
        if status == 404:
            first.setdefault(404, clock())
            return ci_retry.TRANSIENT if clock() - first[404] < PROPAGATION_SECONDS else ci_retry.PERMANENT
        return ci_retry.TRANSIENT if status == 403 else ci_retry.classify(error)

    def kind(error):
        status = getattr(error, "status", None)
        if status == 401:
            return "denied"
        if status == 404 and 404 in first and clock() - first[404] >= PROPAGATION_SECONDS:
            return "untrusted"
        return "unreadable"
    return classify, kind


def inventory_retryable(error):
    return ci_retry.PERMANENT if isinstance(error, Refusal) and str(error) in MALFORMED else ci_retry.TRANSIENT


def newest_finals(api, refs, *, deadline=None, clock=time.monotonic, sleep=time.sleep):
    """Each current lane's newest final source and kind: final, new, unreadable, denied or untrusted.

    Refs come from one live advertisement. Each tag object is read, with
    retries, through the reader allocation uses, then checked as catch-up and
    allocation check it: its name, its commit and its receipt must agree.
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
        classify, kind = object_reader(clock)
        try:
            ci_retry.call(lambda: api.request("git/tags/" + oid, attempts=1), name="release-tag-read",
                          classify=classify, deadline=deadline, clock=clock, sleep=sleep,
                          honour_retry_after=True, **READ_POLICY)
        except UNREADABLE as error:
            result[component] = (None, kind(error))
            continue
        try:
            # The reader keeps the object it read, so these checks see the same bytes.
            result[component] = (final_receipt(api, component, version, oid)["source"], "final")
        except UNREADABLE:
            result[component] = (None, "untrusted")
    return result


def read_finals(root, token, history, *, runner=subprocess.run, seconds=FINAL_SECONDS,
                clock=time.monotonic, sleep=time.sleep):
    """Return the advertised develop head and each lane's newest final.

    A different head makes this plan obsolete only when its first-parent
    history contains the push. Retry a lagging or inconclusive advertisement.
    """
    deadline = clock() + seconds

    def bounded(args, *, timeout, **kwargs):
        remaining = deadline - clock()
        if remaining <= 0:
            raise subprocess.TimeoutExpired(args, timeout)
        return runner(args, timeout=min(timeout, remaining), **kwargs)

    def read_refs():
        refs = advertised(bounded)
        develop = refs["refs/heads/develop"]
        if develop != history.head:
            require(develop not in history.positions, "Develop advertisement predates the push")
            fresh = DevelopHistory.from_head(develop, root=root, runner=bounded, depth=HISTORY_LIMIT)
            require(history.head in fresh.positions, "Develop history has not reached the push")
        return refs

    try:
        refs = ci_retry.call(read_refs, name="release-tag-inventory", classify=inventory_retryable,
                             deadline=deadline, clock=clock, sleep=sleep, **READ_POLICY)
    except UNREADABLE as error:
        kind = "untrusted" if inventory_retryable(error) == ci_retry.PERMANENT else "unreadable"
        return None, dict.fromkeys(DEPLOYABLES, (None, kind))
    develop = refs["refs/heads/develop"]
    if develop != history.head:
        return develop, None
    try:
        api = GitHub(token, runner, local_tags=local_tag_reader(root, deadline=deadline),
                     retry={"clock": clock, "sleep": sleep}, deadline=deadline)
    except UNREADABLE:
        return develop, dict.fromkeys(DEPLOYABLES, (None, "denied"))
    return develop, newest_finals(api, refs, deadline=deadline, clock=clock, sleep=sleep)


def introductions(root, history):
    """Each current lane's newest first-parent addition to the deployable manifest on this history."""
    policy = ContractPolicy(history, root=root)
    names = {}

    def listed(commit):
        if commit not in names:
            present = policy.git("ls-tree", "--name-only", commit, "--", MANIFEST).strip()
            names[commit] = set(json.loads(policy.git("show", commit + ":" + MANIFEST))) if present else set()
        return names[commit]

    wanted, found = set(DEPLOYABLES.values()), {}
    for commit in policy.git("log", "--first-parent", "--max-count=2000", "--format=%H", history.head,
                             "--", MANIFEST).splitlines():
        if commit not in history.positions or not wanted - found.keys():
            break
        header = policy.git("cat-file", "commit", commit).split("\n\n", 1)[0]
        parents = [line[7:] for line in header.splitlines() if line.startswith("parent ")]
        before = listed(parents[0]) if parents else set()
        for name in (wanted - found.keys()) & (listed(commit) - before):
            found[name] = commit
    return {component: found.get(name) for component, name in DEPLOYABLES.items()}


def plan_window(history, adoption, finals, introduced):
    """Choose the window's first revision. Raises PlanRefused when no start can be proved.

    `introduced` maps each lane to its newest manifest addition on this history.
    """
    floor = adoption if adoption in history.positions else None
    commits = {position: sha for sha, position in history.positions.items()}
    bounds, refused = {}, {}
    for component in sorted(DEPLOYABLES):
        source, kind = finals.get(component, (None, "unreadable"))
        if kind in ("unreadable", "denied", "untrusted"):
            refused.setdefault(kind, []).append(component)
            continue
        if kind == "new" and introduced.get(component) in history.positions:
            following = history.positions[introduced[component]] + 1
            if following > 0:
                continue  # Added by this very push: nothing after its addition yet.
            source = commits[following]
        if source in history.positions:
            bounds[component] = history.positions[source], kind
        elif floor is not None:
            bounds[component] = history.positions[floor], "floor"
        else:
            refused.setdefault("floor", []).append(component)
    for kind in ("untrusted", "denied", "floor", "unreadable"):
        if kind in refused:
            raise PlanRefused(kind, refused[kind])
    start = min((position for position, _kind in bounds.values()), default=0)
    lanes = [component if kind == "final" else component + ":" + kind
             for component, (position, kind) in bounds.items() if position == start]
    if floor is not None and history.positions[floor] > start:
        start, lanes = history.positions[floor], ["adoption"]
    added = {component: history.positions[source] for component, source in introduced.items()
             if source in history.positions}
    crossed = sorted(component for component, position in added.items() if position >= start)
    if crossed:
        raise PlanRefused("introduced", crossed, commits[max(added[component] for component in crossed)])
    after = max(added.values(), default=None)
    return Window(commits[start], lanes or ["adoption"], None if after is None else commits.get(after + 1))


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
        line = f"PLAN refused reason={window['refused']} lanes={','.join(window['lanes'])}"
        return line + (f" at={window['at']}" if window.get("at") else "")
    line = (f"PLAN window start={window['start']} revisions={window['revisions']} "
            f"adoption={window['adoption']} bound-by={','.join(window['bound_by'])}")
    return line + (f" transition={window['transition']}" if window.get("transition") else "")


def window_warning(window):
    """An annotation when a window is wide enough to matter, or nearing the history limit."""
    revisions = window.get("revisions")
    if not isinstance(revisions, int) or revisions <= WIDE_REVISIONS:
        return None
    lanes = ",".join(window["bound_by"])
    if revisions > HISTORY_LIMIT - 200:
        return (f"::warning title=Release plan near its history limit::The plan reconciles {revisions} of at most "
                f"{HISTORY_LIMIT} first-parent merges, bound by {lanes}. Beyond the limit that lane needs "
                "CMA_CD_ADOPTION_SHA; setting it to a later merge now also shortens the plan.")
    return (f"::warning title=Wide release plan::The plan reconciles {revisions} first-parent merges, bound by "
            f"{lanes}, whose newest release is that old. A cold selection cache analyses each one; setting "
            "CMA_CD_ADOPTION_SHA to a later merge raises the start.")
