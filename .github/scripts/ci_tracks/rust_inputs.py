"""Identify module files reachable only through explicit cfg(test) edges."""

import re

DECLARATION = re.compile(r'((?:\s*#\[[^\]]+\]\s*)*)(?:pub(?:\([^)]*\))?\s+)?mod\s+(\w+)\s*;')


def test_modules(root, directories):
    graph, roots = {}, set()
    for directory in directories:
        source_root = root / directory / 'src'
        roots.update(path for path in (source_root / 'lib.rs', source_root / 'main.rs') if path.exists())
        roots.update((source_root / 'bin').glob('*.rs'))
        roots.update((source_root / 'bin').glob('*/main.rs'))
        for source in source_root.rglob('*.rs'):
            edges = graph.setdefault(source, [])
            for attrs, name in DECLARATION.findall(source.read_text()):
                explicit = re.search(r'#\[path\s*=\s*"([^"]+)"\]', attrs)
                parent = source.parent if source.stem in {'lib', 'main', 'mod'} else source.with_suffix('')
                candidates = [source.parent / explicit[1]] if explicit else [parent / (name + '.rs'), parent / name / 'mod.rs']
                target = next((p for p in candidates if p.is_file()), None)
                if target is not None:
                    edges.append((target, bool(re.search(r'#\[cfg\(test\)\]', attrs))))

    def reachable(tests):
        found, pending = set(), list(roots)
        while pending:
            path = pending.pop()
            if path in found:
                continue
            found.add(path)
            pending.extend(target for target, gated in graph.get(path, []) if tests or not gated)
        return found

    # Unknown module syntax is retained, as are files also reached by product
    # code. A filename alone never proves that a Rust source file is test-only.
    return {p.relative_to(root).as_posix() for p in reachable(True) - reachable(False)}
