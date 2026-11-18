"""Read only the explicit deployment switch from the immutable private checkout."""
import yaml

from staging_cd_common import require
from staging_cd_targets import APP_TARGETS, RUNTIME_TARGETS, TARGETS


class UniqueMapping(yaml.SafeLoader):
    """Do not accept an ambiguous target switch or YAML merge override."""


def mapping(loader, node):
    result = {}
    for key_node, value_node in node.value:
        key = loader.construct_object(key_node, deep=True)
        require(isinstance(key, str) and key not in result and key != "<<",
                "Private deployment policy is ambiguous")
        result[key] = loader.construct_object(value_node, deep=True)
    return result


UniqueMapping.add_constructor(yaml.resolver.BaseResolver.DEFAULT_MAPPING_TAG, mapping)


def switches(script):
    path = script.parent.parent / "config.yaml"
    require(path.is_file() and not path.is_symlink() and path.stat().st_size <= 262144,
            "Private deployment policy is missing or invalid")
    value = yaml.load(path.read_text(encoding="utf-8"), Loader=UniqueMapping)
    require(isinstance(value, dict) and isinstance(value.get("apps"), dict),
            "Private deployment policy has no targets")
    apps = value["apps"]
    require(set(apps) == {"am", "eg", "web"}, "Private deployment policy target set differs")
    for target in apps.values():
        require(isinstance(target, dict) and target.get("deploy") in ("enabled", "disabled"),
                "Private deployment switch is missing or invalid")
    return {key: target["deploy"] == "enabled" for key, target in apps.items()}


def selected(script, components, *, automatic, mode):
    policy = switches(script)
    require(mode in ("release", "bootstrap") and (not automatic or mode == "release"),
            "Private deployment mode differs")
    require(bool(components) and set(components) <= TARGETS, "Private deployment selection differs")
    enabled = {key: policy["am" if key in RUNTIME_TARGETS else key] for key in components}
    if not automatic:
        require(all(enabled.values()), "Selected deployment is disabled")
    require(mode != "bootstrap" or set(components) == APP_TARGETS, "Bootstrap requires all components")
    return sorted(key for key in components if enabled[key])
