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
    # A test is referenced when its stem occurs in the text of every retained
    # file joined by b'\n'. Rescanning that corpus per test per round is slow,
    # so each round answers the same question exactly from less text:
    # - Tests sharing a stem share the answer, so each stem is searched once.
    # - A match of a stem without b'\n' cannot cross a join separator, so it
    #   lies inside one file. A stem not found yet is absent from every file
    #   retained in earlier rounds, so only the files added since are searched.
    # - Such a match consists of bytes that some stem contains. Turning every
    #   other byte into b'\n' and searching each distinct piece once keeps every
    #   match and creates none.
    # An empty stem, or one containing b'\n' (legal in a file name), can match
    # across a separator, where file adjacency follows set order. It is searched
    # in the corpus joined exactly as before; the sets are built by the same
    # operations as before, so they iterate in the same order.
    stems = {p: Path(p).stem.encode(errors='surrogatepass') for p in tests}
    plain = {s for s in stems.values() if s and b'\n' not in s}
    alphabet = {byte for stem in plain for byte in stem}
    table = bytes(byte if byte in alphabet else ord('\n') for byte in range(256))
    texts, read, found = {}, set(), set()
    while True:
        # Read each file once, in the order the joined corpus lists it.
        added = [p for p in retained if p not in read]
        read.update(added)
        for p in added:
            if Path(p).suffix in text_suffixes:
                # UTF-8 preserves substring matches and keeps a single non-BMP
                # character from widening the entire corpus for every stem search.
                texts[p] = (root / p).read_text(errors='replace').encode()
        pieces = b'\n'.join(texts[p] for p in added if p in texts).translate(table).split(b'\n')
        corpus = b'\n'.join(set(pieces))
        found.update(s for s in plain - found if s in corpus)
        remaining = tests - retained
        spanning = {stems[p] for p in remaining} - plain
        if spanning:
            text = b'\n'.join(texts[p] for p in retained if p in texts)
            found.update(s for s in spanning if s in text)
        referenced = {p for p in remaining if stems[p] in found}
        if not referenced:
            return retained
        retained.update(referenced)
