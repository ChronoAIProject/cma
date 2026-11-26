"""Independent qualification with explicit AM/runtime compatibility boundaries."""
import subprocess

from staging_cd_common import Refusal, matches, require
from staging_cd_compatibility import ContractPolicy, GROUP
from staging_cd_promotion import UNAVAILABLE, proven_components
from staging_cd_receipts import canonical, validate_identity
from staging_cd_reservations import validate_reservation
from staging_cd_resolve import ObsoleteSource
from staging_cd_set import automatic_set, document, later_final, release_set
from staging_cd_targets import TARGETS
from staging_cd_watermark import SUPERSEDED
from staging_cd_transition import verify_transition
from staging_cd_contract import compatibility

REASONS = frozenset({"publication-unqualified", "publication-read-failed", "superseded-source",
                     "contract-group-incomplete", "contract-group-mismatch", "contract-analysis-unavailable"})


def validate_failures(value):
    require(isinstance(value, dict) and set(value) <= TARGETS
            and all(isinstance(reason, str) and reason in REASONS for reason in value.values()),
            "Component failure report is invalid")
    return value


def groups(matrix):
    require(isinstance(matrix, dict) and set(matrix) == {"include"}
            and isinstance(matrix["include"], list) and len(matrix["include"]) <= len(TARGETS),
            "Publication matrix is malformed")
    result = {}
    for entry in matrix["include"]:
        require(isinstance(entry, dict) and set(entry) in ({"component", "tasks"}, {"component", "tasks", "contract_transition"}), "Publication group is malformed")
        component = entry["component"]
        require(isinstance(component, str) and component in TARGETS and component not in result,
                "Publication group is unknown or duplicated")
        if "contract_transition" in entry:
            require(component in GROUP and matches(r"[0-9a-f]{40}", entry["contract_transition"]),
                    "Contract transition marker is invalid")
        tasks = document(entry["tasks"])
        require(isinstance(tasks, list) and bool(tasks), "Publication group is empty")
        for task in tasks:
            require(isinstance(task, dict) and task.get("component") == component
                    and task.get("mode") in ("resume", "publish"), "Publication task is malformed")
            field = "receipt" if task["mode"] == "resume" else "reservation"
            require(set(task) == {"component", "mode", field}, "Publication task fields differ")
            value = task[field]
            require(isinstance(value, dict), "Publication identity is malformed")
            identity = (validate_identity(value.get("identity")) if field == "receipt"
                        else validate_reservation(value))
            require(identity["component"] == component, "Publication task identity differs")
        result[component] = tasks
    return result


def transition_anchor(matrix):
    marked = {e["component"]: e["contract_transition"] for e in matrix["include"] if "contract_transition" in e}
    require(not marked or (marked.keys() == GROUP and len(set(marked.values())) == 1),
            "Contract transition marker coverage differs")
    return next(iter(marked.values()), None)


def overtaken_admission(matrix, value, failures, api):
    """The components of a wholly overtaken independent admission, else none.

    Every member must be superseded-source with a final of the same component
    at a later develop source. That final's own run promotes it in its own lane,
    so this admission deploys nothing and is handled. A contract member, a
    transition, a partial set, missing evidence or any other reason stays red.
    """
    try:
        tasks = groups(matrix)
        if (value is not None or not tasks or transition_anchor(matrix) is not None or GROUP & tasks.keys()
                or failures.keys() != tasks.keys() or set(failures.values()) != {"superseded-source"}):
            return ()
        history = api.history()
        for component, entries in tasks.items():
            last = entries[-1]
            source = (last["receipt"] if last["mode"] == "resume" else last["reservation"])["source"]
            if not later_final(api.tags(component), source, history):
                return ()
        return tuple(sorted(tasks))
    except (Refusal, OSError, ValueError, KeyError, TypeError, subprocess.SubprocessError):
        return ()


def verified_resume(matrix, value, failures, api, read_proofs):
    """Only an exact, wholly qualified resume can reuse a previous deployment."""
    if value is None or failures:
        return False
    if any(task["mode"] != "resume" for tasks in groups(matrix).values() for task in tasks):
        return False
    receipts = value["receipts"]
    try:
        evidence = read_proofs()
        # A contract cohort needs one writer that verified the entire set.
        # Independent receipts cannot stand in for an atomic deployment.
        witnesses = [evidence[component] for component in receipts]
        return (all(row == witnesses[0] for row in witnesses)
                and receipts.keys() <= proven_components(evidence, receipts, api.history()))
    except UNAVAILABLE:
        return False


def automatic_admission(matrix, api, registry, *, policy=None):
    tasks = groups(matrix)  # Malformed shared input never becomes a partial deployment.
    transition = transition_anchor(matrix)
    receipts, failures = {}, {}
    for entry in matrix["include"]:
        component = entry["component"]
        try:
            # The existing strict resolver verifies every expected receipt in
            # this component, including its tail, and rejects superseded sources.
            value = automatic_set({"include": [{"component": component, "tasks": entry["tasks"]}]}, api, registry)
            if component in GROUP:
                compatibility(value["receipts"][component])
            receipts[component] = value["receipts"][component]
        except Refusal as error:
            obsolete = str(error) == SUPERSEDED or isinstance(error, ObsoleteSource)
            failures[component] = "superseded-source" if obsolete else "publication-unqualified"
        except (OSError, ValueError, KeyError, TypeError, subprocess.SubprocessError):
            failures[component] = "publication-read-failed"
    if GROUP & tasks.keys():
        try:
            policy = policy or ContractPolicy(api.history())
            for component, source in policy.required_sources(tasks).items():
                tasks[component] = []  # Required member, not a fabricated publication.
                try:
                    finals = [tag for tag in api.tags(component) if tag['target'] == source]
                    require(len(finals) == 1, "Contract peer is not finalized at the transition source")
                    task = {"component": component, "mode": "resume", "receipt": finals[0]["receipt"]}
                    entry = {"component": component, "tasks": canonical([task])}
                    value = automatic_set({"include": [entry]}, api, registry)
                    compatibility(value["receipts"][component])
                    receipts[component] = value["receipts"][component]
                    tasks[component] = [task]
                except (OSError, ValueError, KeyError, TypeError, subprocess.SubprocessError):
                    failures[component] = "publication-unqualified"
            reason = policy.group_reason(tasks, receipts)
            if transition and not reason:
                if GROUP <= receipts.keys():
                    verify_transition(transition, receipts, policy)
                else:
                    reason = "contract-group-incomplete"
            require(reason is None or reason in REASONS, "Compatibility decision is invalid")
        except (OSError, ValueError, KeyError, TypeError, subprocess.SubprocessError):
            reason = "contract-analysis-unavailable"
        if reason:
            for component in GROUP & tasks.keys():
                receipts.pop(component, None)
                failures.setdefault(component, reason)
    selected = release_set(receipts, transition=transition if GROUP <= receipts.keys() else None) if receipts else None
    return selected, validate_failures(failures)
