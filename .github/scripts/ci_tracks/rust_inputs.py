"""Prove cfg(test)-only module edges from a Rust syntax tree."""

from functools import lru_cache

from syntax import RUST, descendants, parse, string_value

# Keep the reuse inside one selector process and bound retained source bytes.
MAX_CACHED_SOURCE_BYTES = 64 * 1024


def attributes(items):
    gated, path, ambiguous = False, None, False
    for item in items:
        attr = item.named_children[0]
        name = attr.named_children[0].text.decode()
        if name == 'cfg':
            tokens = attr.named_children[1:]
            # Only this exact parsed attribute proves a test-only edge. More
            # complex cfg expressions are retained conservatively.
            gated |= len(tokens) == 1 and tokens[0].text.decode().replace(' ', '') == '(test)'
        elif name == 'path':
            try:
                path = string_value(attr.named_children[-1])
            except ValueError:
                ambiguous = True
        elif name == 'cfg_attr':
            ambiguous = True
    return gated and not ambiguous, path, ambiguous


def module_declarations(container, directory=(), inherited=False):
    attrs = []
    for item in container.named_children:
        if item.type in {'line_comment', 'block_comment'}:
            continue
        if item.type == 'attribute_item':
            attrs.append(item)
            continue
        if item.type != 'mod_item':
            attrs = []
            continue
        gated, explicit, ambiguous = attributes(attrs)
        attrs = []
        name = item.child_by_field_name('name').text.decode()
        body = item.child_by_field_name('body')
        if body is not None:
            yield from module_declarations(body, (*directory, name), inherited or gated)
        else:
            yield directory, name, explicit, inherited or gated, ambiguous


def declarations(data):
    try:
        tree = parse(data, RUST)
        # A node's text is a slice of the source. Without these bytes no
        # include invocation can exist, even inside a nested expression.
        if b'include' in data and any(
                node.type == 'macro_invocation' and node.child_by_field_name('macro').text == b'include'
                for node in descendants(tree)):
            raise ValueError('item include needs explicit input modeling')
        return tuple(module_declarations(tree))
    except ValueError:
        return None


cached_declarations = lru_cache(maxsize=12000)(declarations)


def source_edges(source):
    data = source.read_bytes()
    modules = (cached_declarations(data) if len(data) <= MAX_CACHED_SOURCE_BYTES
               else declarations(data))
    if modules is None:
        return None
    # Parsing depends on the exact bytes. Module resolution also depends on
    # the available files, so each snapshot resolves those paths again.
    parent = source.parent if source.stem in {'lib', 'main', 'mod'} else source.with_suffix('')
    edges = []
    for relative, name, explicit, gated, ambiguous in modules:
        directory = parent.joinpath(*relative)
        candidates = ([source.parent / explicit] if explicit
                      else [directory / (name + '.rs'), directory / name / 'mod.rs'])
        matches = [path for path in candidates if path.is_file()]
        if len(matches) != 1 or ambiguous:
            return None
        edges.append((matches[0], gated))
    return edges


def test_modules(root, directories, source_cache=None):
    # Consumers may share parsed module edges only while their snapshot is
    # immutable. Neither paths nor mtimes are a cross-snapshot cache identity.
    source_cache = {} if source_cache is None else source_cache
    graph, roots, uncertain, package_files = {}, set(), set(), {}
    for directory in directories:
        source_root = root / directory / 'src'
        roots.update(path for path in (source_root / 'lib.rs', source_root / 'main.rs') if path.exists())
        roots.update((source_root / 'bin').glob('*.rs'))
        roots.update((source_root / 'bin').glob('*/main.rs'))
        files = set(source_root.rglob('*.rs'))
        package_files[directory] = files
        for source in files:
            if source not in source_cache:
                source_cache[source] = source_edges(source)
            if source_cache[source] is None:
                uncertain.add(source)
            else:
                graph[source] = source_cache[source]

    def reachable(tests):
        found, pending = set(), list(roots)
        while pending:
            path = pending.pop()
            if path in found:
                continue
            found.add(path)
            pending.extend(target for target, gated in graph.get(path, []) if tests or not gated)
        return found

    production = reachable(False)
    opaque = set().union(*(files for files in package_files.values() if files & uncertain & production))
    return {p.relative_to(root).as_posix() for p in reachable(True) - production - opaque}
