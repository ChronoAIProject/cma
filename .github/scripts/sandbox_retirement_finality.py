"""Decide the expected provider disposition from fresh exact-object reads."""

from enum import Enum


class Finality(Enum):
    SETTLED = "settled"
    PENDING = "pending"
    REPLACED = "replaced"
    INVALID = "invalid"


def read_identity(expected, observed):
    if observed is None:
        return None
    if not isinstance(observed, dict) or not isinstance(observed.get("metadata"), dict):
        return Finality.INVALID
    metadata = observed["metadata"]
    if not metadata.get("uid") or metadata.get("name") != expected["name"]:
        return Finality.INVALID
    if metadata["uid"] != expected["uid"]:
        return Finality.REPLACED
    return Finality.PENDING if metadata.get("deletionTimestamp") else Finality.SETTLED


def evaluate_storage(expected, pvc, pv):
    pvc_state = read_identity(expected["pvc"], pvc)
    pv_state = read_identity(expected["pv"], pv)
    for state in (pvc_state, pv_state):
        if state is Finality.INVALID or state is Finality.REPLACED:
            return state
    if pv_state is not Finality.SETTLED:
        return Finality.PENDING
    if pvc is not None and pvc["metadata"].get("namespace") != expected["namespace"]:
        return Finality.INVALID
    policy = pv.get("spec", {})
    claim = policy.get("claimRef", {})
    if (
        policy.get("persistentVolumeReclaimPolicy") != "Retain"
        or claim.get("uid") != expected["pvc"]["uid"]
        or claim.get("name") != expected["pvc"]["name"]
        or claim.get("namespace") != expected["namespace"]
    ):
        return Finality.INVALID
    phase = pv.get("status", {}).get("phase")
    if expected["storage"] == "retain_bound":
        return (
            Finality.SETTLED
            if pvc_state is Finality.SETTLED
            and pvc.get("spec", {}).get("volumeName") == expected["pv"]["name"]
            and pvc.get("status", {}).get("phase") == "Bound"
            and phase == "Bound"
            else Finality.PENDING
        )
    if expected["storage"] == "retain_released":
        return Finality.SETTLED if pvc is None and phase == "Released" else Finality.PENDING
    return Finality.INVALID


def owned_by_sandbox(pod, expected):
    return any(
        owner.get("apiVersion") == "agents.x-k8s.io/v1beta1"
        and owner.get("kind") == "Sandbox"
        and owner.get("uid") == expected["sandbox"]["uid"]
        for owner in pod["metadata"].get("ownerReferences", [])
    )


def matches_preimage(expected, observed):
    for kind in ("sandbox", "claim"):
        obj = observed[kind]
        if read_identity(expected[kind], obj) is not Finality.SETTLED:
            return False
        if obj["metadata"].get("namespace") != expected["namespace"]:
            return False
    approved = {pod["name"]: pod for pod in expected["pods"]}
    seen = set()
    for pod in observed["pods"]:
        name = pod["metadata"]["name"]
        if name in approved:
            if (
                read_identity(approved[name], pod) is not Finality.SETTLED
                or pod["metadata"].get("namespace") != expected["namespace"]
                or not owned_by_sandbox(pod, expected)
            ):
                return False
            seen.add(name)
        elif owned_by_sandbox(pod, expected):
            return False
    return seen == approved.keys() and evaluate_storage(
        {**expected, "storage": "retain_bound"}, observed["pvc"], observed["pv"],
    ) is Finality.SETTLED


def evaluate(expected, observed):
    """Object 404s and a complete namespace Pod list prove runtime absence."""
    try:
        pending = False
        for name in ("sandbox", "claim"):
            state = read_identity(expected[name], observed[name])
            if state is Finality.INVALID or state is Finality.REPLACED:
                return state
            pending |= state is not None
        pods = observed["pods"]
        if not isinstance(pods, list):
            return Finality.INVALID
        approved_pods = {pod["name"]: pod for pod in expected["pods"]}
        for pod in pods:
            meta = pod["metadata"]
            if meta["namespace"] != expected["namespace"] or not meta["uid"]:
                return Finality.INVALID
            if original := approved_pods.get(meta["name"]):
                state = read_identity(original, pod)
                if state is Finality.INVALID or state is Finality.REPLACED:
                    return state
                pending = True
            if owned_by_sandbox(pod, expected):
                pending = True
        storage = evaluate_storage(expected, observed["pvc"], observed["pv"])
        if storage is Finality.SETTLED and pending:
            return Finality.PENDING
        return storage
    except (KeyError, TypeError, AttributeError):
        return Finality.INVALID
