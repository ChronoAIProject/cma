"""Shared syntax parsers; source is never evaluated as executable code."""

from tree_sitter import Language, Parser
import tree_sitter_rust
import tree_sitter_typescript

RUST = Language(tree_sitter_rust.language())
TYPESCRIPT = Language(tree_sitter_typescript.language_typescript())


def parse(text, language):
    tree = Parser(language).parse(text.encode() if isinstance(text, str) else text)
    if tree.root_node.has_error:
        raise ValueError('source syntax cannot be analyzed safely')
    return tree.root_node


def descendants(node):
    yield node
    for child in node.named_children:
        yield from descendants(child)


def string_value(node):
    if node.type not in {'string', 'string_literal'}:
        raise ValueError('a literal string is required')
    # Escaped/computed paths need explicit modeling; never infer an exclusion.
    if any(child.type not in {'string_fragment', 'string_content'} for child in node.named_children):
        raise ValueError('escaped or dynamic string is unsupported')
    return node.text.decode()[1:-1]
