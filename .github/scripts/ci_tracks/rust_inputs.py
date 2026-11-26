"""Prove cfg(test)-only module edges from a Rust syntax tree."""

from functools import lru_cache

from syntax import RUST, descendants, parse, string_value

# Keep the reuse inside one selector process and bound retained source bytes.
MAX_CACHED_SOURCE_BYTES = 64 * 1024
INCLUDE_MACROS = {b'include', b'include_str', b'include_bytes'}
COMMENTS = {'line_comment', 'block_comment'}
LINT_ATTRIBUTES = {b'allow', b'expect', b'warn', b'deny', b'forbid'}


def tokens(node):
    return [child for child in node.children if child.type not in COMMENTS]


def arguments(tree):
    if tree.type != 'token_tree':
        raise ValueError('expected Rust macro arguments')
    args = tokens(tree)[1:-1]
    if args and args[-1].type == ',':
        args.pop()
    return args


def include_path(tree):
    args = arguments(tree)
    if len(args) == 1:
        return 'source', string_value(args[0])
    if len(args) == 3 and args[0].text == b'concat' and args[1].type == '!':
        expr = arguments(args[2])
        if len(expr) == 5 and expr[0].text == b'env' and expr[1].type == '!' and expr[3].type == ',':
            env = arguments(expr[2])
            if len(env) == 1 and string_value(env[0]) == 'CARGO_MANIFEST_DIR':
                suffix = string_value(expr[4])
                if suffix.startswith('/') and len(suffix) > 1:
                    return 'manifest', suffix[1:]
    raise ValueError('computed include path is unsupported')


def included_files(data):
    if b'include' not in data:
        return ()
    result = []
    for node in descendants(parse(data, RUST)):
        if node.type == 'macro_invocation':
            macro = tokens(node)[0].text.rsplit(b'::', 1)[-1].strip()
            if macro in INCLUDE_MACROS:
                result.append((macro, *include_path(tokens(node)[-1])))
        if node.type == 'token_tree':
            # Rust's syntax tree leaves nested macro bodies as tokens.
            items = tokens(node)
            for name, bang, tree in zip(items, items[1:], items[2:]):
                if name.type == 'identifier' and name.text in INCLUDE_MACROS and bang.type == '!':
                    ancestor = node.parent
                    while ancestor:
                        if ancestor.type == 'macro_definition':
                            raise ValueError('include from a macro definition is unsupported')
                        ancestor = ancestor.parent
                    result.append((name.text, *include_path(tree)))
    return result


def conditional_lint_only(attr):
    tree = attr.child_by_field_name('arguments')
    if tree is None:
        return False
    args = arguments(tree)
    # A single built-in lint attribute cannot gate or redirect a module.
    return (len(args) >= 4 and args[-3].type == ','
            and sum(item.type == ',' for item in args) == 1
            and args[-2].type == 'identifier' and args[-2].text in LINT_ATTRIBUTES
            and args[-1].type == 'token_tree' and args[-1].children[0].type == '(')


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
        elif name == 'cfg_attr' and not conditional_lint_only(attr):
            ambiguous = True
    return gated and not ambiguous, path, ambiguous


def module_declarations(container, directory=(), inherited=False):
    attrs = []
    for item in container.named_children:
        if item.type in COMMENTS:
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
