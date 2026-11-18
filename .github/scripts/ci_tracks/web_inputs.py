"""Keep Vite public assets and build references before test-source exclusions."""

from pathlib import Path, PurePosixPath

from docker_inputs import matches
from syntax import TYPESCRIPT, descendants, parse, string_value


def public_directory(root):
    configs = [p for p in (root / 'web').glob('vite.config.*') if p.suffix in {'.ts', '.js', '.mts', '.mjs', '.cts', '.cjs'}]
    if not configs:
        return 'web/public'
    if len(configs) != 1:
        raise ValueError('ambiguous Vite configuration')
    tree = parse(configs[0].read_bytes(), TYPESCRIPT)
    if not any(node.type == 'object' for node in descendants(tree)):
        raise ValueError('Vite configuration has no static object to inspect')
    if any(node.type == 'spread_element' and node.parent.type == 'object' for node in descendants(tree)):
        raise ValueError('spread Vite configuration needs explicit public-directory modeling')
    public, base = [], []
    for node in descendants(tree):
        if node.type != 'pair':
            continue
        key = node.child_by_field_name('key').text.decode().strip('\"\'')
        value = node.child_by_field_name('value')
        if key == 'publicDir':
            public.append(False if value.type == 'false' else string_value(value))
        elif key == 'root':
            base.append(string_value(value))
    if len(public) > 1 or len(base) > 1:
        raise ValueError('conditional Vite public/root configuration needs modeling')
    directory = public[0] if public else 'public'
    if directory is False:
        return None
    path = PurePosixPath('web') / (base[0] if base else '.') / directory
    if path.is_absolute() or '..' in path.parts:
        raise ValueError('Vite public directory must be repository-local')
    return path.as_posix().rstrip('/')


def web_inputs(root, files, exclusions):
    candidates = {p for p in files if p.startswith('web/')}
    public = public_directory(root)
    assets = {p for p in candidates if public and (p == public or p.startswith(public + '/'))}
    # Exclusions apply to proven source/test corpora, never copied public assets
    # or arbitrary binary assets named like a test.
    source_suffixes = {'.ts', '.tsx', '.js', '.jsx', '.mjs', '.cjs', '.json'}
    tests = {p for p in candidates - assets if
             (p.startswith(('web/e2e/', 'web/src/', 'web/build/')) and Path(p).suffix in source_suffixes
              and any(matches(p[4:], pattern) for pattern in exclusions)) or p in {'web/README', 'web/README.md'}}
    text_suffixes = source_suffixes | {'.css', '.scss', '.sass', '.less', '.html', '.svg'}
    retained = candidates - tests
    # Follow references transitively: a retained fixture can itself import a
    # second test-named file. CSS/HTML references count as well as JS imports.
    while True:
        text = '\n'.join((root / p).read_text(errors='replace') for p in retained if Path(p).suffix in text_suffixes)
        referenced = {p for p in tests - retained if Path(p).stem in text}
        if not referenced:
            return retained
        retained.update(referenced)
