"""Analyze contiguous first-parent changes once per immutable source snapshot."""

from concurrent.futures import ProcessPoolExecutor
import hashlib
import json
import multiprocessing
import os
from pathlib import Path
import sys
import time

import selection
from selection import analyze_comparison, check_sha, recipes, snapshot, summarize
from git_comparison import comparison_details
from image_inputs import NAMES, validate_recipe_sources
from selection_cache import SelectionCache, canonical, fingerprint
from uncertainty import conservative


def summarize_revision(root, revision, fallback, cache, context):
    inputs = cache.load_inputs(revision, context) if cache else None
    with snapshot(root, revision) as tree:
        if inputs is None:
            current = summarize(tree, fallback)
            if cache:
                cache.save_inputs(revision, context, current[1])
            return current
        # Cached maps replace syntax/input analysis only. Ownership and exact
        # reviewed Dockerfile hashes are still validated from immutable source.
        graph, manifest = selection.workspace(tree), recipes(tree, fallback)
        validate_recipe_sources(tree, manifest)
        return graph, inputs, manifest


def workers():
    # Parallel summaries failed on hosted runners after 7-12 minutes (runs
    # 44680314976 and later), most likely concurrent `cargo metadata --locked`
    # calls contending for Cargo's package cache lock within their 120 s limit.
    # Stay sequential inside the larger budget until that path is qualified.
    return 1


def expired(deadline):
    return deadline is not None and time.monotonic() >= deadline


def summarize_many(root, revisions, fallback, cache, context, parallel=None, deadline=None):
    """Summarize revisions in history order, starting no new snapshot after deadline.

    Each summary saves its input map as soon as it exists, so work done before
    the deadline is kept for the next invocation even when this one stops.
    """
    parallel = workers() if parallel is None else parallel
    if parallel <= 1 or len(revisions) <= 1:
        summaries = {}
        for revision in revisions:
            if expired(deadline):
                break
            summaries[revision] = summarize_revision(root, revision, fallback, cache, context)
        return summaries
    # Cache records are one file per revision, checksummed on load. Workers
    # write disjoint revisions; their hit counters stay in the children.
    with ProcessPoolExecutor(max_workers=min(parallel, len(revisions)),
                             mp_context=multiprocessing.get_context('fork')) as pool:
        if deadline is None:
            futures = {revision: pool.submit(summarize_revision, root, revision, fallback, cache, context)
                       for revision in revisions}
            return {revision: future.result() for revision, future in futures.items()}
        # With a deadline, submit one wave per worker count and check between waves.
        summaries = {}
        for start in range(0, len(revisions), parallel):
            if expired(deadline):
                break
            wave = revisions[start:start + parallel]
            futures = {revision: pool.submit(summarize_revision, root, revision, fallback, cache, context)
                       for revision in wave}
            summaries.update({revision: future.result() for revision, future in futures.items()})
        return summaries


class BudgetExhausted(Exception):
    """The deadline stopped analysis; every finished decision and input map was saved."""

    def __init__(self, decided, total):
        super().__init__(decided, total)
        self.decided, self.total = decided, total


def determine_many(root, revisions, cache=None, parallel=None, deadline=None):
    if not isinstance(revisions, list) or not revisions or len(revisions) > 2000:
        raise ValueError('invalid comparison batch')
    if len(set(check_sha(revision) for revision in revisions)) != len(revisions):
        raise ValueError('duplicate comparison identity')
    fallback = recipes(root)
    # git archive can omit a tracked manifest via export-ignore. Bind both
    # kinds of records to the fallback actually available to these snapshots.
    context = hashlib.sha256(canonical(fallback)).hexdigest() if cache else None
    # Resolve every comparison and cached result first, then summarize each
    # missing snapshot once. A commit's summary is its successor's before-state.
    plan, previous = [], None
    for head in revisions:
        base, _head, reason = comparison_details(root, {'after': head}, 'push', allow_shallow_parent=True)
        if previous is not None and base != previous:
            raise ValueError('comparison batch is not contiguous first-parent history')
        cached = cache.load(base, head, context) if cache is not None and reason is None else None
        plan.append((base, head, reason, cached))
        previous = head
    needed = []
    for base, head, reason, cached in plan:
        if cached is None:
            needed.extend(([base] if reason is None else []) + [head])
    summaries = summarize_many(root, list(dict.fromkeys(needed)), fallback, cache, context, parallel, deadline)
    results = []
    for base, head, reason, cached in plan:
        if cached is not None:
            results.append(cached)
            continue
        if head not in summaries or (reason is None and base not in summaries):
            continue  # Its snapshot was not reached before the deadline.
        current = summaries[head]
        result = (conservative(NAMES, reason, current[0]) if reason else
                  analyze_comparison(root, base, head, [summaries[base], current]))
        result.update(base=base, head=head)
        results.append(result)
        if cache is not None:
            cache.save(result, context)
    if len(results) != len(plan):
        # Never a partial success: the caller learns only how far it got. The
        # saved records stay unpruned so the next invocation continues.
        raise BudgetExhausted(len(results), len(plan))
    if cache is not None:
        cache.retain(revisions)
    return results


# A batch stopped by its budget exits with this status and prints no decisions.
INCOMPLETE = 3


def main():
    started = time.monotonic()
    stopped = None
    try:
        if len(sys.argv) not in (3, 4):
            raise ValueError('expected repository, revision batch and optional budget seconds')
        deadline = None
        if len(sys.argv) == 4:
            if not sys.argv[3].isdigit() or not 0 < int(sys.argv[3]) <= 86400:
                raise ValueError('invalid selection budget')
            deadline = started + int(sys.argv[3])
        data = Path(sys.argv[2]).read_bytes()
        if len(data) > 100000:
            raise ValueError('comparison batch exceeds limit')
        root, cache = Path(sys.argv[1]).resolve(), None
        if os.environ.get('CD_SELECTOR_CACHE'):
            context = json.loads(os.environ['CD_SELECTOR_CACHE'])
            if set(context) != {'directory', 'policy'} or context['policy'] != fingerprint(root):
                raise ValueError('selector cache policy differs')
            cache = SelectionCache(Path(context['directory']), context['policy'])
        try:
            result = determine_many(root, json.loads(data), cache, deadline=deadline)
        except BudgetExhausted as error:
            stopped = error
    except Exception:  # Source-controlled errors must never echo file contents.
        print('::error::Cannot establish complete image comparison batch.', file=sys.stderr)
        return 1
    stats = {'event': 'selection-cache'}
    if cache is not None:
        stats.update(hits=cache.hits, misses=cache.misses, input_hits=cache.input_hits,
                     input_misses=cache.input_misses)
    stats['elapsed_ms'] = int((time.monotonic() - started) * 1000)
    if stopped is not None:
        stats.update(decided=stopped.decided, total=stopped.total)
        print(json.dumps(stats), file=sys.stderr)
        return INCOMPLETE
    if cache is not None:
        print(json.dumps(stats), file=sys.stderr)
    print(json.dumps(result, sort_keys=True))
    return 0


if __name__ == '__main__':
    raise SystemExit(main())
