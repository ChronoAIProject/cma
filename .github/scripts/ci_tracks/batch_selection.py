"""Analyze contiguous first-parent changes once per immutable source snapshot."""

import json
import os
from pathlib import Path
import sys

from selection import analyze_summaries, changed_paths, check_sha, git, recipes, snapshot, summarize
from selection_cache import SelectionCache, fingerprint


def determine_many(root, revisions, cache=None):
    if not isinstance(revisions, list) or not revisions or len(revisions) > 2000:
        raise ValueError('invalid comparison batch')
    if len(set(check_sha(revision) for revision in revisions)) != len(revisions):
        raise ValueError('duplicate comparison identity')
    fallback = recipes(root)
    previous, previous_summary, results = None, None, []
    for head in revisions:
        base = check_sha(git(root, 'rev-parse', head + '^1').decode().strip())
        if previous is not None and base != previous:
            raise ValueError('comparison batch is not contiguous first-parent history')
        cached = cache.load(base, head) if cache is not None else None
        if cached is not None:
            results.append(cached)
            previous, previous_summary = head, None
            continue
        if previous_summary is None:
            with snapshot(root, base) as tree:
                previous_summary = summarize(tree, fallback)
        with snapshot(root, head) as tree:
            current = summarize(tree, fallback)
        result = analyze_summaries(changed_paths(root, base, head), [previous_summary, current])
        result.update(base=base, head=head)
        results.append(result)
        if cache is not None:
            cache.save(result)
        # Retain only immutable summaries, never extracted trees or external
        # cache state. A commit's summary is its successor's before-state.
        previous, previous_summary = head, current
    if cache is not None:
        cache.retain(revisions)
    return results


def main():
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
        print(json.dumps({'event': 'selection-cache', 'hits': cache.hits, 'misses': cache.misses}), file=sys.stderr)
    print(json.dumps(result, sort_keys=True))
    return 0


if __name__ == '__main__':
    raise SystemExit(main())
