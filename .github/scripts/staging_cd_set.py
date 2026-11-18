"""Freeze complete, verified public release sets before deployment admission."""
import json

from staging_cd_common import require
from staging_cd_receipts import canonical, validate_receipt
from staging_cd_reservations import validate_reservation
from staging_cd_resolve import components, resolve, verify
from staging_cd_versions import DEPLOYABLES, version_tuple

from staging_cd_targets import APP_TARGETS, RUNTIME_TARGETS, TARGETS
from staging_cd_runtime_smoke import validate_smoke


def document(text):
    require(isinstance(text, str) and len(text) <= 65536, "Release input is oversized or missing")
    def unique(pairs):
        result = {}
        for key, value in pairs:
            require(key not in result, "Release input contains duplicate fields")
            result[key] = value
        return result
    return json.loads(text, object_pairs_hook=unique)


def selection(inputs):
    selected = components(inputs.get("deployables"))
    mode = inputs.get("mode", "release")
    require(mode in ("release", "bootstrap"), "Deployment mode is invalid")
    require(mode != "bootstrap" or set(selected) == APP_TARGETS, "Bootstrap requires the complete component set")
    versions, version = inputs.get("versions", ""), inputs.get("version", "")
    require(isinstance(versions, str) and isinstance(version, str), "Version selection is malformed")
    require(not (versions and version), "Version inputs are mutually exclusive")
    if versions:
        mapping = document(versions)
        require(isinstance(mapping, dict) and set(mapping) == set(selected), "Version map must exactly cover the selected set")
        for value in mapping.values():
            version_tuple(value)
    elif version:
        require(len(selected) == 1, "Single-version input requires one component")
        version_tuple(version)
        mapping = {selected[0]: version}
    else:
        mapping = {component: "latest" for component in selected}
    return mode, mapping


def validate_set(value):
    require(isinstance(value, dict) and set(value) == {"schema", "receipts"}
            and value["schema"] == "cma-deployment-set-v1", "Deployment set is malformed")
    receipts = value["receipts"]
    require(isinstance(receipts, dict) and bool(receipts) and set(receipts) <= TARGETS,
            "Deployment component set is invalid")
    for component, receipt in receipts.items():
        validate_receipt(receipt)
        require(receipt["identity"]["component"] == component, "Deployment receipt component differs")
        if component in RUNTIME_TARGETS:
            validate_smoke(receipt.get("runtime_smoke"), source=receipt["source"],
                           digest=receipt["digest"], archive=receipt["archive"])
    return value


def release_set(receipts):
    return validate_set({"schema": "cma-deployment-set-v1", "receipts": receipts})


def dispatch_set(inputs, api, registry):
    mode, mapping = selection(inputs)
    receipts = {component: resolve(component, version, api.tags(component), registry, api.history())
                for component, version in mapping.items()}
    return mode, release_set(receipts)


def automatic_set(matrix, api, registry):
    require(isinstance(matrix, dict) and set(matrix) == {"include"}
            and isinstance(matrix["include"], list), "Publication matrix is malformed")
    receipts, seen = {}, set()
    for entry in matrix["include"]:
        require(isinstance(entry, dict) and set(entry) == {"component", "tasks"}, "Publication group is malformed")
        component = entry["component"]
        require(component in DEPLOYABLES and component not in seen, "Publication group is unknown or duplicated")
        seen.add(component)
        tasks = document(entry["tasks"])
        require(isinstance(tasks, list) and bool(tasks), "Publication group is empty")
        if component not in TARGETS:
            continue
        tags = api.tags(component)
        # Finalization may preserve a successful prefix after a failed push.
        # Require every expected final receipt before admitting any member.
        for task in tasks:
            require(isinstance(task, dict) and task.get("component") == component, "Publication task component differs")
            if task.get("mode") == "resume":
                expected = validate_receipt(task["receipt"])
                receipt = resolve(component, expected["identity"]["version"], tags, registry, api.history())
                require(receipt == expected, "Resumed publication receipt changed")
            else:
                require(task.get("mode") == "publish", "Publication task mode differs")
                reserved = validate_reservation(task["reservation"])
                receipt = resolve(component, reserved["version"], tags, registry, api.history())
                expected = dict(reserved, schema="cma-image-release-v1", build=receipt["identity"]["build"])
                require(receipt["identity"] == expected, "Final receipt differs from its reservation")
        receipts[component] = verify(receipt, tags, registry, api.history(), automatic=True)
    return release_set(receipts) if receipts else None


def verify_set(value, api, registry, *, automatic, inputs=None):
    validate_set(value)
    receipts = value["receipts"]
    if not automatic:
        _, mapping = selection(inputs)
        require(set(receipts) == set(mapping), "Dispatch set changed after resolution")
        require(all(version == "latest" or receipts[key]["identity"]["version"] == version
                    for key, version in mapping.items()), "Dispatch version changed after resolution")
    for component, receipt in receipts.items():
        verify(receipt, api.tags(component), registry, api.history(), automatic=automatic)
    return value


def private_document(value):
    validate_set(value)
    result = {"schema": 1, "components": {
        component: {"image": receipt["repository"] + "@" + receipt["digest"],
                    "source_sha": receipt["source"], "version": receipt["identity"]["version"],
                    **({"runtime_smoke": receipt["runtime_smoke"]} if component in RUNTIME_TARGETS else {})}
        for component, receipt in value["receipts"].items()}}
    require(len(canonical(result).encode()) <= 8192, "Private release input is oversized")
    return result
