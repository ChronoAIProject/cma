"""Resume only exact fixture creation intents; never replace an existing object."""
import copy
import json
import sys
from pathlib import Path

from authority import (SPEC_LABEL, create_resource, fingerprint, guard_mutation,
                       guard_resource, identity, kubectl, record_created,
                       require, save_plan, validate_plan)


def get_resource(plan, desired):
    args = ["get", desired["kind"], desired["metadata"]["name"], "--ignore-not-found", "-o", "json"]
    if "namespace" in desired["metadata"]:
        args += ["-n", desired["metadata"]["namespace"]]
    return kubectl(plan, args)


def resume_creation(plan_path, plan, resources):
    validate_plan(plan)
    for desired in resources:
        guard_resource(plan, desired)
        current = get_resource(plan, desired)
        prior = [entry for entry in plan["resources"] if all(entry[key] == value for key, value in identity(desired).items())]
        if prior:
            require(current is not None, "journaled_resource_disappeared")
            namespace = kubectl(plan, ["get", "namespace", plan["namespace"], "-o", "json"])
            guard_mutation(plan, namespace, current)
            continue
        if current:
            # The first create can have succeeded after its client timed out.
            # Recover its exact request, never mistake matching names for proof.
            guard_resource(plan, current)
            namespace = current if desired["kind"] == "Namespace" else kubectl(plan, ["get", "namespace", plan["namespace"], "-o", "json"])
            if desired["kind"] != "Namespace":
                guard_mutation(plan, namespace, namespace)
            intent_path = Path(plan_path).parent / ("intent-" + desired["kind"].lower() + "-" + desired["metadata"]["name"] + ".json")
            require(intent_path.exists(), "unrecorded_create_intent")
            intent = json.loads(intent_path.read_text())
            expected = {"identity": identity(desired), "spec_hash": fingerprint(desired)[:63]}
            require(intent == expected, "create_intent_mismatch")
            require(current["metadata"]["labels"].get(SPEC_LABEL) == expected["spec_hash"], "ambiguous_resource_spec_mismatch")
            record_created(plan, current)
            save_plan(plan_path, plan)
        else:
            create_resource(plan_path, plan, copy.deepcopy(desired))
        print("Settled", desired["kind"], desired["metadata"]["name"], flush=True)


def main():
    require(len(sys.argv) == 2, "usage_private_run_directory")
    directory = Path(sys.argv[1]).resolve()
    plan_path = directory / "plan.json"
    plan = json.loads(plan_path.read_text())
    resources = json.loads((directory / "bootstrap-private.json").read_text())
    resume_creation(plan_path, plan, resources)


if __name__ == "__main__":
    try:
        main()
    except Exception as error:
        print("Fixture resume failed:", str(error) if isinstance(error, ValueError) else type(error).__name__, file=sys.stderr)
        raise SystemExit(1)
