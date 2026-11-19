"""Source-derived compatibility evidence for immutable AM/runtime publications."""
import json
from pathlib import Path

from staging_cd_common import matches, require
from staging_cd_compatibility import GROUP, RECIPE, VERSION_FILE, runtime_labels

SCHEMA = "cma-runtime-compatibility-v1"
VERSIONS = ("runtime_contract", "bootstrap", "home_layout")
OCI_LABELS = dict(zip(VERSIONS, ("org.chronoai.cma.runtime-contract",
                               "org.chronoai.cma.bootstrap", "org.chronoai.cma.home-layout")))
RECIPE_LABELS = dict(zip(VERSIONS, ("dev.chrono.cma.runtime-contract",
                                  "dev.chrono.cma.runtime-bootstrap", "dev.chrono.cma.home-layout")))


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


def declared_versions(text):
    require(isinstance(text, str) and len(text.encode()) <= 1024,
            "Runtime compatibility declaration is oversized")
    def unique(pairs):
        result = {}
        for key, value in pairs:
            require(key not in result, "Runtime compatibility declaration has duplicate fields")
            result[key] = value
        return result
    value = json.loads(text, object_pairs_hook=unique)
    require(isinstance(value, dict) and set(value) == {"schema", *VERSIONS}
            and value["schema"] == "cma-runtime-versions-v1"
            and all(matches(r"[1-9][0-9]{0,4}", value[key]) and int(value[key]) <= 65535
                    for key in VERSIONS), "Runtime compatibility declaration is invalid")
    return {key: value[key] for key in VERSIONS}


def derive(component, source, read):
    if component not in GROUP:
        return None
    # Cargo generates the actual AM admission constants from this same strict
    # declaration. Rust comments, cfg branches and unrelated code are not inputs.
    versions = declared_versions(read(VERSION_FILE))
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
