"""Freeze complete, verified public release sets before deployment admission."""
import json

from cd_target import PRODUCTION, STAGING
from staging_cd_common import Refusal, matches, require
from staging_cd_compatibility import ContractPolicy, GROUP
from staging_cd_watermark import SUPERSEDED, eligible_receipts
from staging_cd_transition import verify_transition
from staging_cd_receipts import canonical, sha256, validate_receipt
from staging_cd_reservations import validate_reservation
from staging_cd_resolve import ObsoleteSource, components, resolve, verify
from staging_cd_versions import DEPLOYABLES, version_tuple

from staging_cd_targets import APP_TARGETS, RUNTIME_TARGETS, TARGETS
from staging_cd_runtime_smoke import validate_smoke


OVERTAKEN = "superseded_by_newer_final"
ALREADY_VERIFIED = "already-verified"
# A first staging attempt refused before any change for a transient cause; its
# promotion re-admits and deploys once more, and that attempt's outcome counts.
RETRY_PENDING = "retry-pending"


class OvertakenByNewerFinal(Refusal):
    """Every member of an independent automatic set has a newer final at a later develop source.

    The run that finalized each newer version promotes it in its own lane: it
    deploys it, is overtaken the same way, or fails. So this set deploys nothing
    and is not a failure. A contract group member can be finalized in a lane
    that never promotes it, so a set that holds one still fails.
    """

    def __init__(self, components):
        super().__init__("Automatic deployment set was overtaken by newer finals")
        self.components = tuple(sorted(components))


def later_final(tags, source, history):
    """True when a final of this component sits at a later develop first-parent source."""
    position = history.position(source)
    return any(history.position(tag["target"]) > position for tag in tags)


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
    require(isinstance(value, dict) and set(value) in ({"schema", "receipts"}, {"schema", "receipts", "contract_transition"})
            and value["schema"] == "cma-deployment-set-v1", "Deployment set is malformed")
    receipts = value["receipts"]
    require(isinstance(receipts, dict) and bool(receipts) and set(receipts) <= TARGETS,
            "Deployment component set is invalid")
    if "contract_transition" in value:
        require(matches(r"[0-9a-f]{40}", value["contract_transition"]) and GROUP <= receipts.keys(),
                "Transition deployment requires its complete group")
    for component, receipt in receipts.items():
        validate_receipt(receipt)
        require(receipt["identity"]["component"] == component, "Deployment receipt component differs")
        if component in RUNTIME_TARGETS:
            validate_smoke(receipt.get("runtime_smoke"), source=receipt["source"],
                           digest=receipt["digest"], archive=receipt["archive"])
    return value


def release_set(receipts, *, transition=None):
    value = {"schema": "cma-deployment-set-v1", "receipts": receipts}
    if transition is not None:
        value["contract_transition"] = transition
    return validate_set(value)


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
    if receipts:
        api.require_current_versions(receipts)
        return release_set(receipts)
    return None


def verify_set(value, api, registry, *, automatic, inputs=None):
    validate_set(value)
    receipts = value["receipts"]
    if not automatic:
        require("contract_transition" not in value, "Dispatch cannot assume automatic transition ownership")
        _, mapping = selection(inputs)
        require(set(receipts) == set(mapping), "Dispatch set changed after resolution")
        require(all(version == "latest" or receipts[key]["identity"]["version"] == version
                    for key, version in mapping.items()), "Dispatch version changed after resolution")
    obsolete, overtaken = set(), set()
    for component, receipt in receipts.items():
        tags, history = api.tags(component), api.history()
        try:
            verify(receipt, tags, registry, history, automatic=automatic)
        except ObsoleteSource:
            if not automatic:
                raise
            obsolete.add(component)
            # Positive evidence: a final of this component at a later develop source.
            if later_final(tags, receipt["source"], history):
                overtaken.add(component)
    if automatic:
        transition = value.get("contract_transition")
        if transition:
            verify_transition(transition, receipts, ContractPolicy(api.history()))
        try:
            current = eligible_receipts(receipts, api.require_current_versions,
                                        lambda: ContractPolicy(api.history()),
                                        obsolete=obsolete, transition=transition)
        except Refusal as error:
            # Only a set whose every member a newer final overtook is handed to
            # those finals' runs. Any other refusal, or a contract member, fails.
            if (str(error) == SUPERSEDED and not transition and not GROUP & receipts.keys()
                    and overtaken == receipts.keys()):
                raise OvertakenByNewerFinal(overtaken) from error
            raise
        return release_set(current, transition=transition if GROUP <= current.keys() else None)
    return value


def private_document(value, *, target=STAGING):
    validate_set(value)
    require(target in (STAGING, PRODUCTION), "Deployment target is invalid")
    from staging_cd_contract import GROUP, compatibility
    for component, receipt in value["receipts"].items():
        if component in GROUP:
            compatibility(receipt)
    # A controller must explicitly support each protected request schema.
    continuity = value["receipts"].get("web", {}).get("asset_continuity") if target == STAGING else None
    schema = 2 if GROUP & value["receipts"].keys() else 1
    if continuity is not None:
        # Automatic CD already promotes Web in its own lane. An explicit mixed
        # set cannot interleave another component with the fenced Web phases.
        require(set(value["receipts"]) == {"web"}, "Web continuity requires a standalone staging release")
        schema = 3
    result = {"schema": schema, "components": {
        component: {"image": receipt["repository"] + "@" + receipt["digest"],
                    "source_sha": receipt["source"], "version": receipt["identity"]["version"],
                    **({"publication": receipt} if component in GROUP else {}),
                    **({"runtime_smoke": receipt["runtime_smoke"],
                        "runtime_bootstrap": {"schema": "cma-runtime-bootstrap-request-v1",
                                              "publication": sha256(receipt)}}
                       if component in RUNTIME_TARGETS else {})}
        for component, receipt in value["receipts"].items()}}
    if continuity is not None:
        result["components"]["web"]["asset_continuity"] = continuity
    require(len(canonical(result).encode()) <= 8192, "Private release input is oversized")
    return result
