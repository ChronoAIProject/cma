"""Pure, fixed-error plans for the owner-approved Secret cutover; never log objects."""
import base64
import copy
import datetime as dt
import hmac
import uuid

NAMESPACE = "chrono-bot"
NAME = "cma-event-gateway"
SECRET = "cma-event-gateway-owner-keys"
KEYS = ("CMAEG_OWNER_KEY_SERVICE_ID", "CMAEG_OWNER_KEY_CUTOVER_ID")
LAST_APPLIED = "kubectl.kubernetes.io/last-applied-configuration"


class Refused(Exception):
    """The message is a fixed reason, never object content or an upstream error."""


def require(condition, reason):
    if not condition:
        raise Refused(reason)


def identity(obj, kind, name):
    require(isinstance(obj, dict), "invalid_object")
    meta = obj.get("metadata", {})
    require(obj.get("kind") == kind and meta.get("name") == name
            and meta.get("namespace") == NAMESPACE, "wrong_object_identity")
    require(meta.get("deletionTimestamp") is None, "object_terminating")
    require(all(isinstance(meta.get(k), str) and meta[k] for k in ("uid", "resourceVersion")),
            "missing_object_fence")
    return {k: meta[k] for k in ("uid", "resourceVersion")}


def container(deployment):
    identity(deployment, "Deployment", NAME)
    items = deployment["spec"]["template"]["spec"]["containers"]
    matches = [(i, item) for i, item in enumerate(items) if item.get("name") == "cmaeg"]
    require(len(matches) == 1, "wrong_container")
    i, item = matches[0]
    require(isinstance(item.get("image"), str) and "@sha256:" in item["image"], "unpinned_image")
    require(item.get("envFrom") == [{"configMapRef": {"name": NAME}}], "unexpected_env_from")
    require(isinstance(item.get("env"), list), "missing_env")
    return i, item


def references():
    return [{"name": key, "valueFrom": {"secretKeyRef": {
        "name": SECRET, "key": key, "optional": True}}} for key in KEYS]


def reference_state(deployment):
    _, item = container(deployment)
    entries = [entry for entry in item["env"] if entry.get("name") in KEYS]
    require(not entries or sorted(entries, key=lambda e: e["name"]) ==
            sorted(references(), key=lambda e: e["name"]), "unexpected_owner_env")
    require(not any(entry.get("name") in ("CMAEG_OWNER_KEYS_ENABLED",
                "CMAEG_WORKER_CREDENTIAL_TRACKING_ENABLED") for entry in item["env"]),
            "unexpected_owner_flag_override")
    return bool(entries)


def valid_pair(pair):
    require(set(pair) == set(KEYS), "missing_identifier")
    for value in pair.values():
        require(isinstance(value, str) and len(value) == 36, "invalid_identifier")
        try:
            parsed = uuid.UUID(value)
        except (ValueError, AttributeError):
            raise Refused("invalid_identifier") from None
        require(parsed.int != 0 and str(parsed) == value, "invalid_identifier")
    return pair


def config_pair(configmap):
    identity(configmap, "ConfigMap", NAME)
    data = configmap.get("data", {})
    require(data.get("CMAEG_OWNER_KEYS_ENABLED") == "true"
            and data.get("CMAEG_WORKER_CREDENTIAL_TRACKING_ENABLED") == "true",
            "owner_key_mode_not_enabled")
    require(not configmap.get("binaryData"), "unexpected_binary_config")
    present = {key: data[key] for key in KEYS if key in data}
    return valid_pair(present) if present else None


def secret_pair(secret):
    identity(secret, "Secret", SECRET)
    require(secret.get("type") == "Opaque" and set(secret.get("data", {})) == set(KEYS),
            "unexpected_secret_shape")
    try:
        pair = {k: base64.b64decode(secret["data"][k], validate=True).decode("utf-8") for k in KEYS}
    except (ValueError, UnicodeError, TypeError):
        raise Refused("invalid_secret_data") from None
    return valid_pair(pair)


def same_values(configmap, secret):
    pair = config_pair(configmap)
    require(pair is not None, "config_identifiers_already_removed")
    other = secret_pair(secret)
    require(all(hmac.compare_digest(pair[k], other[k]) for k in KEYS), "secret_value_mismatch")


def projection(state, context):
    cm, dep, secret = state
    config_pair(cm)
    refs = reference_state(dep)
    i, item = container(dep)
    generation = dep["metadata"].get("generation")
    require(type(generation) is int and generation > 0, "invalid_generation")
    result = {"context": context, "namespace": NAMESPACE,
              "configmap": identity(cm, "ConfigMap", NAME),
              "deployment": {**identity(dep, "Deployment", NAME), "generation": generation,
                             "container_index": i, "image": item["image"]},
              "secret": None, "references_present": refs}
    if secret is not None:
        secret_pair(secret)
        result["secret"] = identity(secret, "Secret", SECRET)
    return result


def tests(obj, kind, name):
    fence = identity(obj, kind, name)
    return [{"op": "test", "path": "/metadata/" + key, "value": value}
            for key, value in fence.items()]


def seed_plan(configmap, secret):
    pair = config_pair(configmap)
    require(pair is not None, "config_identifiers_already_removed")
    if secret is not None:
        same_values(configmap, secret)
        return None
    return {"apiVersion": "v1", "kind": "Secret", "metadata": {
        "name": SECRET, "namespace": NAMESPACE, "labels": {
            "app.kubernetes.io/name": NAME, "app.kubernetes.io/part-of": "cma"}},
        "type": "Opaque", "data": {key: base64.b64encode(pair[key].encode()).decode() for key in KEYS}}


def switch_plan(configmap, deployment, secret):
    same_values(configmap, secret)
    if reference_state(deployment):
        return None
    i, item = container(deployment)
    prefix = f"/spec/template/spec/containers/{i}"
    return tests(deployment, "Deployment", NAME) + [
        {"op": "test", "path": "/metadata/generation", "value": deployment["metadata"]["generation"]},
        {"op": "test", "path": prefix + "/name", "value": "cmaeg"},
        {"op": "test", "path": prefix + "/image", "value": item["image"]},
        {"op": "replace", "path": prefix + "/env", "value": copy.deepcopy(item["env"]) + references()}]


def ready(deployment):
    spec, status = deployment["spec"], deployment.get("status", {})
    desired = spec.get("replicas")
    return (type(desired) is int and desired > 0
            and status.get("observedGeneration") == deployment["metadata"]["generation"]
            and all(status.get(k) == desired for k in ("replicas", "updatedReplicas", "readyReplicas", "availableReplicas"))
            and status.get("unavailableReplicas", 0) == 0)


def verify_receipt(receipt, deployment, secret, now=None):
    require(isinstance(receipt, dict), "invalid_verification_receipt")
    require(receipt.get("deployment_uid") == deployment["metadata"]["uid"]
            and receipt.get("generation") == deployment["metadata"]["generation"]
            and receipt.get("secret") == identity(secret, "Secret", SECRET), "verification_fence_mismatch")
    require(all(receipt.get(k) is True for k in ("owner_key_read_passed", "existing_channels_passed",
                                               "observation_passed")), "verification_incomplete")
    try:
        verified = dt.datetime.fromisoformat(receipt["verified_at"].replace("Z", "+00:00"))
        age = ((now or dt.datetime.now(dt.timezone.utc)) - verified).total_seconds()
    except (ValueError, TypeError, KeyError, AttributeError):
        raise Refused("invalid_verification_time") from None
    require(0 <= age <= 1800, "verification_expired")


def cleanup_plan(configmap, deployment, secret, receipt):
    require(reference_state(deployment) and ready(deployment), "rollout_not_verified")
    verify_receipt(receipt, deployment, secret)
    pair = config_pair(configmap)
    patch = tests(configmap, "ConfigMap", NAME)
    if pair is not None:
        same_values(configmap, secret)
        patch += [{"op": "remove", "path": "/data/" + key} for key in KEYS]
    annotation = configmap["metadata"].get("annotations", {}).get(LAST_APPLIED, "")
    if any(key in annotation for key in KEYS):
        patch.append({"op": "remove", "path": "/metadata/annotations/" + LAST_APPLIED.replace("/", "~1")})
    return patch if len(patch) > 2 else None
