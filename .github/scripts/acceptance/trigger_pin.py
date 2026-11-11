"""Reviewed Trigger identity, public policy, and immutable Launch revision."""

from identity import canonical_hash, fingerprint
from safety import Refusal, read_private


def configured(p):
    value = read_private("acceptance-trigger.json", {})
    pin = p.approval["trigger"]
    if (pin["name"] != "nightly-smoke-readonly"
            or fingerprint(value.get("public_id", "")) != pin["public_id_sha256"]):
        raise Refusal("trigger_identity_not_reviewed_nightly_fixture")
    descriptor(p, value.get("public_descriptor", {}))
    return value


def descriptor(p, value):
    pin = p.approval["trigger"]
    policy = pin["policy"]
    technical = value.get("technical", {})
    if (canonical_hash(value) != pin["descriptor_sha256"]
            or value.get("availability") != "published"
            or value.get("always_review") is not policy["always_review"]
            or value.get("first_message") != policy["first_message"]
            or value.get("trust", {}).get("disclosures") != policy["disclosures"]
            or any(technical.get(k) != policy[k] for k in ("provider", "effort", "services"))):
        raise Refusal("trigger_policy_not_reviewed_nightly_fixture")


def launch(p, value):
    pin = p.approval["trigger"]
    if (fingerprint(value.get("public_id", "")) != pin["public_id_sha256"]
            or value.get("revision_digest") != pin["revision_digest"]
            or value.get("trust", {}).get("disclosures") != []):
        raise Refusal("launch_revision_or_policy_not_reviewed")
