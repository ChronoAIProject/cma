"""Source-derived compatibility evidence for immutable AM/runtime publications."""
import re
from pathlib import Path

from staging_cd_common import matches, require
from staging_cd_compatibility import GROUP, RECIPE, SOURCES, runtime_labels

SCHEMA = "cma-runtime-compatibility-v1"
VERSIONS = ("runtime_contract", "bootstrap", "home_layout")
OCI_LABELS = dict(zip(VERSIONS, ("org.chronoai.cma.runtime-contract",
                               "org.chronoai.cma.bootstrap", "org.chronoai.cma.home-layout")))
RECIPE_LABELS = dict(zip(VERSIONS, ("dev.chrono.cma.runtime-contract",
                                  "dev.chrono.cma.runtime-bootstrap", "dev.chrono.cma.home-layout")))
POOL_SOURCE = "api/crates/layer3-adapter/src/agent_sandbox/pool/model.rs"


def validate(value, source):
    require(isinstance(value, dict) and set(value) == {"schema", "source", *VERSIONS}
            and value["schema"] == SCHEMA and value["source"] == source
            and matches(r"[0-9a-f]{40}", source)
            and all(matches(r"[1-9][0-9]{0,4}", value[key]) for key in VERSIONS),
            "Runtime compatibility evidence is absent or malformed")
    return value


def compatibility(receipt):
    from staging_cd_receipts import validate_receipt
    validate_receipt(receipt)
    value = validate(receipt.get("compatibility"), receipt["source"])
    return tuple(value[key] for key in VERSIONS)


def parse(text):
    # Reuse the pinned Rust parser. Regex over raw Rust could mistake comments,
    # macro bodies or conditional declarations for active contract authority.
    from tree_sitter import Language, Parser
    import tree_sitter_rust
    root = Parser(Language(tree_sitter_rust.language())).parse(text.encode()).root_node
    require(not root.has_error, "Runtime compatibility source cannot be parsed")
    return root


def descendants(node):
    yield node
    for child in node.named_children:
        yield from descendants(child)


def literal_constant(text, name, kind):
    root = parse(text)
    declarations = [node for node in descendants(root) if node.type == "const_item"
                    and node.child_by_field_name("name").text.decode() == name]
    require(len(declarations) == 1 and declarations[0].parent == root,
            "Runtime version declaration is ambiguous")
    node = declarations[0]
    previous = node.prev_named_sibling
    while previous and previous.type in {"line_comment", "block_comment"}:
        previous = previous.prev_named_sibling
    require(previous is None or previous.type != "attribute_item",
            "Conditional runtime version declaration is unsupported")
    require(not any(child.type == "inner_attribute_item" for child in root.named_children),
            "Conditional runtime version module is unsupported")
    value = node.child_by_field_name("value")
    pattern = r'"([1-9][0-9]{0,4})"' if kind == "string_literal" else r"([1-9][0-9]{0,4})"
    match = re.fullmatch(pattern, value.text.decode()) if value is not None else None
    require(value is not None and value.type == kind and match is not None,
            "Runtime version must be an explicit literal")
    return match[1]


def home_layout(text):
    root = parse(text)
    # Model only the actual label comparison in the AM pool validation. A
    # source refactor must update this derivation, rather than guess a version.
    comparisons = []
    for node in descendants(root):
        if node.type != "binary_expression":
            continue
        value = re.sub(r"\s+", "", node.text.decode())
        match = re.fullmatch(
            r'self\.image\.labels\.get\(crate::runtime_bootstrap::RUNTIME_HOME_LAYOUT_LABEL\)'
            r'\.map\(String::as_str\)!=Some\("([1-9][0-9]{0,4})"\)', value)
        if match:
            comparisons.append(match[1])
    require(len(comparisons) == 1, "AM home-layout requirement is unmodeled")
    return comparisons[0]


def derive(component, source, read):
    if component not in GROUP:
        return None
    versions = {"runtime_contract": literal_constant(read(SOURCES[0]), "RUNTIME_CONTRACT_VERSION", "string_literal"),
                "bootstrap": literal_constant(read(SOURCES[1]), "RUNTIME_BOOTSTRAP_VERSION", "integer_literal"),
                "home_layout": home_layout(read(POOL_SOURCE))}
    declared = dict(runtime_labels(read(RECIPE)))
    require(all(declared[RECIPE_LABELS[key]] == version for key, version in versions.items()),
            "AM requirements and runtime image declarations disagree")
    return validate(dict(schema=SCHEMA, source=source, **versions), source)


def from_source(component, source, root):
    return derive(component, source, lambda path: (Path(root) / path).read_text())


def labels(value):
    if value is None:
        return {}
    validate(value, value.get("source"))
    return {label: value[key] for key, label in OCI_LABELS.items()}


def verify_labels(found, value, component):
    validate(value, value.get("source"))
    require(isinstance(found, dict) and all(found.get(key) == version for key, version in labels(value).items()),
            "Image compatibility labels differ from its source receipt")
    if component != "am":
        require(all(found.get(label) == value[key] for key, label in RECIPE_LABELS.items()),
                "Runtime image capability labels differ from its receipt")
