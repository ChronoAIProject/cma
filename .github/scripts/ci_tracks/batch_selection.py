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
from selection import analyze_summaries, changed_paths, check_sha, recipes, snapshot, summarize
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
    # Each summary extracts and analyzes one immutable snapshot. They are
    # independent, so a cold policy can use every runner core.
    return max(1, min(8, os.cpu_count() or 1))


def summarize_many(root, revisions, fallback, cache, context, parallel=None):
    parallel = workers() if parallel is None else parallel
    if parallel <= 1 or len(revisions) <= 1:
        return {revision: summarize_revision(root, revision, fallback, cache, context) for revision in revisions}
    # Cache records are one file per revision, checksummed on load. Workers
    # write disjoint revisions; their hit counters stay in the children.
    with ProcessPoolExecutor(max_workers=min(parallel, len(revisions)),
                             mp_context=multiprocessing.get_context('fork')) as pool:
        futures = {revision: pool.submit(summarize_revision, root, revision, fallback, cache, context)
                   for revision in revisions}
        return {revision: future.result() for revision, future in futures.items()}


def determine_many(root, revisions, cache=None, parallel=None):
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
    summaries = summarize_many(root, list(dict.fromkeys(needed)), fallback, cache, context, parallel)
    results = []
    for base, head, reason, cached in plan:
        if cached is not None:
            results.append(cached)
            continue
        current = summaries[head]
        result = (conservative(NAMES, reason, current[0]) if reason else
                  analyze_summaries(changed_paths(root, base, head), [summaries[base], current]))
        result.update(base=base, head=head)
        results.append(result)
        if cache is not None:
            cache.save(result, context)
    if cache is not None:
        cache.retain(revisions)
    return results


def main():
    started = time.monotonic()
    try:
        if len(sys.argv) != 3:
            raise ValueError('expected repository and revision batch')
        data = Path(sys.argv[2]).read_bytes()
        if len(data) > 100000:
            raise ValueError('comparison batch exceeds limit')
        root, cache = Path(sys.argv[1]).resolve(), None
        if os.environ.get('CD_SELECTOR_CACHE'):
            context = json.loads(os.environ['CD_SELECTOR_CACHE'])
            if set(context) != {'directory', 'policy'} or context['policy'] != fingerprint(root):
                raise ValueError('selector cache policy differs')
            cache = SelectionCache(Path(context['directory']), context['policy'])
        result = determine_many(root, json.loads(data), cache)
    except Exception:  # Source-controlled errors must never echo file contents.
        print('::error::Cannot establish complete image comparison batch.', file=sys.stderr)
        return 1
    if cache is not None:
        print(json.dumps({'event': 'selection-cache', 'hits': cache.hits, 'misses': cache.misses,
                          'input_hits': cache.input_hits, 'input_misses': cache.input_misses,
                          'elapsed_ms': int((time.monotonic() - started) * 1000)}), file=sys.stderr)
    print(json.dumps(result, sort_keys=True))
    return 0


if __name__ == '__main__':
    raise SystemExit(main())
