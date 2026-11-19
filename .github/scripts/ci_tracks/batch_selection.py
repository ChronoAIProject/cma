"""Analyze contiguous first-parent changes once per immutable source snapshot."""

import hashlib
import json
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


def determine_many(root, revisions, cache=None):
    if not isinstance(revisions, list) or not revisions or len(revisions) > 2000:
        raise ValueError('invalid comparison batch')
    if len(set(check_sha(revision) for revision in revisions)) != len(revisions):
        raise ValueError('duplicate comparison identity')
    fallback = recipes(root)
    # git archive can omit a tracked manifest via export-ignore. Bind both
    # kinds of records to the fallback actually available to these snapshots.
    context = hashlib.sha256(canonical(fallback)).hexdigest() if cache else None
    previous, previous_summary, results = None, None, []
    for head in revisions:
        base, _head, reason = comparison_details(root, {'after': head}, 'push', allow_shallow_parent=True)
        if previous is not None and base != previous:
            raise ValueError('comparison batch is not contiguous first-parent history')
        cached = cache.load(base, head, context) if cache is not None and reason is None else None
        if cached is not None:
            results.append(cached)
            previous, previous_summary = head, None
            continue
        if previous_summary is None and reason is None:
            previous_summary = summarize_revision(root, base, fallback, cache, context)
        current = summarize_revision(root, head, fallback, cache, context)
        result = (conservative(NAMES, reason, current[0]) if reason else
                  analyze_summaries(changed_paths(root, base, head), [previous_summary, current]))
        result.update(base=base, head=head)
        results.append(result)
        if cache is not None:
            cache.save(result, context)
        # Retain only immutable summaries, never extracted trees. A commit's
        # summary is its successor's before-state.
        previous, previous_summary = head, current
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
