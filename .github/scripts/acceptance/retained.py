"""Hash-only retained-fixture ledger; quotas outlive replaceable campaigns."""

import time

from identity import canonical_hash, fingerprint
from safety import Refusal, decode, read_private, write_private

NAME = "retained-fixtures.json"


def load(p):
    ledger = read_private(NAME)
    subjects = {a: p.approval["actors"][a]["subject_sha256"] for a in ("a", "b")}
    if (not ledger or ledger.get("version") != 1 or ledger.get("subjects") != subjects
            or ledger.get("complete_history") is not True):
        raise Refusal("retained_fixture_history_missing_or_uncertain")
    return ledger


def kind(intent):
    if (intent["method"], intent["path"]) == ("POST", "/api/v1/documents"):
        return "document"
    if (intent["method"], intent["path"]) == ("POST", "/api/v1/usage/metrics/reports"):
        return "report"
    if intent["method"] == "PATCH" and intent["path"].startswith("/api/v1/documents/"):
        return "edit"
    return None


def reserve(p, actor, key, intent):
    category = kind(intent)
    if not category:
        return
    ledger = load(p); slot = actor + ":" + category
    binding = {"key_sha256": fingerprint(key), "intent_sha256": canonical_hash(intent)}
    entry = ledger["slots"].get(slot)
    if entry:
        if category in ("document", "edit"):
            fixture = p.state["fixtures"].get("document_" + actor, {})
            if entry["status"] != "retained" or not fixture.get("id"):
                raise Refusal("consumed_document_slot_requires_existing_fixture")
            check_fixture(p, actor, "document", fixture["id"])
        if any(entry.get(k) != v for k, v in binding.items()):
            raise Refusal("retained_fixture_cap_requires_original_operation")
        if entry["status"] == "pending" and not 0 <= time.time() - entry["created_at"] < 23 * 3600:
            raise Refusal("uncertain_fixture_outside_safe_receipt_window")
        return
    if category in ("document", "edit") and not intent["body"].get("title", "").startswith("CMA acceptance"):
        # Older approved fixtures used this equally explicit label.
        if not intent["body"].get("title", "").startswith("CMA automated acceptance"):
            raise Refusal("retained_document_requires_acceptance_label")
    ledger["slots"][slot] = dict(binding, status="pending", created_at=int(time.time()))
    write_private(NAME, ledger)


def validate(p, actor, key, intent, dispatch=True):
    entry = load(p)["slots"].get(actor + ":" + str(kind(intent)), {})
    if (not key or entry.get("key_sha256") != fingerprint(key)
            or entry.get("intent_sha256") != canonical_hash(intent)):
        raise Refusal("retained_write_not_reserved")
    if dispatch and kind(intent) in ("document", "edit") and entry.get("status") != "pending":
        raise Refusal("consumed_document_slot_forbids_dispatch")


def observed(p, actor, key, intent, status, body):
    category = kind(intent)
    if not category:
        return
    validate(p, actor, key, intent, dispatch=False)
    ledger = load(p); entry = ledger["slots"][actor + ":" + category]
    if status in (200, 201, 202) and isinstance(body, dict):
        resource_id = body.get("report_id") if category == "report" else body.get("id")
        if not isinstance(resource_id, str):
            raise Refusal("retained_fixture_receipt_missing_identity")
        entry.update(status="retained", resource_sha256=fingerprint(resource_id))
        if category == "report":
            expiry = body.get("expires_at")
            if not isinstance(expiry, int):
                raise Refusal("report_expiration_missing")
            entry["expires_at"] = expiry
    # Refusals also occupy the slot; no new key or extra immutable revision.
    elif 400 <= status < 500 and isinstance(body, dict) and body.get("code") not in {
        "idempotency_in_flight", "idempotency_recovery_required", "idempotency_key_reused",
    }:
        entry["status"] = "refused"
    write_private(NAME, ledger)


def check_fixture(p, actor, category, value):
    entry = load(p)["slots"].get(actor + ":" + category, {})
    if entry.get("resource_sha256") != fingerprint(value):
        raise Refusal("fixture_not_in_private_retention_ledger")


def preflight_documents(p):
    """Prove both retained fixtures before either actor can mutate a document."""
    slots = load(p)["slots"]
    for actor in ("a", "b"):
        entry = slots.get(actor + ":document")
        fixture = p.state["fixtures"].get("document_" + actor)
        if entry is None:
            if fixture or actor + ":edit" in slots:
                raise Refusal("document_fixture_history_inconsistent")
            continue
        if entry.get("status") != "retained" or not fixture or not fixture.get("id"):
            raise Refusal("consumed_document_slot_requires_existing_fixture")
        check_fixture(p, actor, "document", fixture["id"])
        response = p.request(actor, "/api/v1/documents/" + fixture["id"])
        if response[0] != 200:
            raise Refusal("retained_document_absent_or_unavailable_no_replacement")
        current = decode(response[2])
        if (current.get("id") != fixture["id"]
                or current.get("owner_subject") != p.accounts[actor]["subject"]
                or current.get("revision") != fixture["revision"]):
            raise Refusal("retained_document_identity_or_revision_changed")


def source_slot_consumed(p, actor):
    return actor + ":edit" in load(p)["slots"]


def reconcile(p):
    for slot, entry in load(p)["slots"].items():
        if entry["status"] != "pending":
            continue
        actor, category = slot.split(":")
        matches = [(label, w) for label, w in p.state["writes"].items()
                   if fingerprint(w["key"]) == entry["key_sha256"]]
        if len(matches) != 1:
            raise Refusal("uncertain_retained_fixture_requires_original_private_state")
        label, write = matches[0]; intent = write["intent"]
        response = p.write(label, actor, intent["path"], intent["method"], intent["body"])
        if response[0] in (200, 201, 202):
            value = decode(response[2])
            fixture = "report_" + actor if category == "report" else "document_" + actor
            p.fixture(fixture, {"report_id": value["report_id"]} if category == "report" else value)


def verify_expiration(p):
    for actor in ("a", "b"):
        entry = load(p)["slots"].get(actor + ":report", {})
        if entry.get("status") != "retained":
            continue
        fixture = p.state["fixtures"].get("report_" + actor, {})
        resource_id = fixture.get("report_id", "")
        check_fixture(p, actor, "report", resource_id)
        response = p.request(actor, "/api/v1/usage/metrics/reports/" + resource_id)
        if response[0] == 404 and int(time.time()) >= entry["expires_at"]:
            p.record("retained_report_expired_" + actor, "PASS", [1320], response)
            ledger = load(p); ledger["slots"][actor + ":report"]["expiry_verified"] = True
            write_private(NAME, ledger)
        elif response[0] in (200, 202):
            # A completed report gets a new logical expiration from the worker.
            expiry = decode(response[2]).get("expires_at")
            if not isinstance(expiry, int) or expiry <= int(time.time()):
                raise Refusal("report_expiration_contract_unconfirmed")
            ledger = load(p); ledger["slots"][actor + ":report"]["expires_at"] = expiry
            write_private(NAME, ledger)
            p.record("retained_report_expiration_" + actor, "BLOCKED", [1320],
                     reason="ttl_declared_but_not_yet_elapsed; no_replacement_report_admitted")
        else:
            raise Refusal("report_expiration_read_unavailable")
