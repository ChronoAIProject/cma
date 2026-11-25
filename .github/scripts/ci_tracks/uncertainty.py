"""Fixed conservative-selection reasons, separate from integrity refusals."""

REASONS = frozenset({'input-analysis-uncertain', 'shallow-history',
                     'comparison-boundary-not-found', 'unknown-input-path',
                     'release-base-unmodeled'})


class SelectionIntegrityError(ValueError):
    """An invalid identity or reviewed input must not become a successful plan."""


def conservative(names, reason, graph=None, count=0):
    if __package__:
        from .policy import select
    else:
        from policy import select
    if reason not in REASONS:
        raise SelectionIntegrityError('invalid fallback reason')
    value = select([], graph, force_all=True)
    value.update(reasons=[reason], deployables=sorted(names), changed_path_count=count)
    value['test_tracks'] = {key: value[key] for key in ('repo', 'am', 'eg', 'web')}
    return value


def unknown_paths(paths, graphs):
    # Known repository automation and public documentation can select no image.
    # Anything outside the modeled roots, or an unowned package, cannot prove
    # a deployable unaffected. Old and new graphs both count for removed paths.
    root_files = {'README.md', 'CLAUDE.md', 'AGENTS.md', 'CONTRIBUTING.md', 'LICENSE', 'NOTICE', '.gitignore', '.dockerignore'}
    for path in paths:
        if path.startswith('api/crates/'):
            if not any(path.startswith(directory + '/') or path == directory
                       for graph in graphs for directory in graph.directories):
                return True
        elif path not in root_files and not path.startswith(('api/', 'web/', '.github/')):
            return True
    return False
