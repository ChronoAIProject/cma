"""Account-wide launch history survives campaign replacement and uncertainty."""

import datetime

from identity import canonical_hash
from safety import Refusal, read_private, write_private

NAME = "launch-ledger.json"
PENDING = {"reserved", "submitted", "resolved"}


def utc_now():
    return datetime.datetime.now(datetime.timezone.utc)


def day(now):
    return now.astimezone(datetime.timezone(datetime.timedelta(hours=8))).date().isoformat()


def load(p):
    ledger = read_private(NAME)
    expected = {a: p.approval["actors"][a]["subject_sha256"] for a in ("a", "b")}
    if (not ledger or ledger.get("version") != 1 or ledger.get("complete_history") is not True
            or ledger.get("subjects") != expected or not isinstance(ledger.get("entries"), dict)):
        raise Refusal("launch_history_missing_or_uncertain")
    for key, entry in ledger["entries"].items():
        if (entry.get("key") != key or entry.get("actor") not in expected
                or entry.get("subject_sha256") != expected[entry["actor"]]
                or entry.get("intent_sha256") != canonical_hash(entry.get("intent"))
                or entry.get("status") not in PENDING | {"cleaned", "refused"}
                or day(datetime.datetime.fromisoformat(entry["created_at"])) != entry.get("day")):
            raise Refusal("launch_history_integrity_failed")
    return ledger


def reserve(p, actor, key, intent, now=None):
    now = now or utc_now()
    ledger = load(p)
    existing = ledger["entries"].get(key)
    if existing:
        validate(p, actor, key, intent, now)
        return
    if intent["body"] != {"start_if_ready": False}:
        raise Refusal("new_launch_must_be_nonstarting_draft")
    entries = [e for e in ledger["entries"].values() if e["actor"] == actor]
    if any(e["status"] in PENDING for e in entries):
        raise Refusal("pending_launch_must_reconcile_before_new_intent")
    if sum(e["day"] == day(now) for e in entries) >= p.approval["limits"]["launch_attempts_per_actor_sgt_day"]:
        raise Refusal("visitor_daily_launch_budget")
    ledger["entries"][key] = {
        "key": key, "actor": actor, "subject_sha256": p.approval["actors"][actor]["subject_sha256"],
        "trigger_sha256": p.approval["trigger"]["public_id_sha256"],
        "day": day(now), "created_at": now.isoformat(), "campaign": p.state["campaign"],
        "intent": intent, "intent_sha256": canonical_hash(intent), "status": "reserved",
    }
    write_private(NAME, ledger)


def validate(p, actor, key, intent, now=None):
    now = now or utc_now()
    entry = load(p)["entries"].get(key)
    if (not entry or entry["actor"] != actor or entry["intent"] != intent
            or entry["trigger_sha256"] != p.approval["trigger"]["public_id_sha256"]):
        raise Refusal("launch_reservation_not_bound_to_exact_request")
    age = (now - datetime.datetime.fromisoformat(entry["created_at"])).total_seconds()
    # REST operation receipts live 24h. Never risk re-creating an expired receipt.
    if entry["status"] in ("reserved", "submitted") and not 0 <= age < 23 * 3600:
        raise Refusal("uncertain_launch_outside_safe_receipt_window")
    return entry


def submitted(p, key):
    ledger = load(p); entry = ledger["entries"][key]
    if entry["status"] not in ("cleaned", "refused"):
        entry["status"] = "submitted"
        write_private(NAME, ledger)


def observed(p, key, status, body):
    ledger = load(p); entry = ledger["entries"][key]
    if entry["status"] in ("cleaned", "refused"):
        return
    entry["response_status"] = status
    entry["response"] = body
    if status in (201, 202) and isinstance(body, dict) and isinstance(body.get("id"), str):
        entry["status"] = "resolved"
        entry["launch_id"] = body["id"]
    elif status in (401, 403, 404, 409, 429) and isinstance(body, dict) and body.get("code") in {
        "not_authenticated", "forbidden", "trigger_not_found",
        "sandbox_credential_setup_required", "resource_quota_exceeded",
    }:
        entry["status"] = "refused"
    else:
        entry["status"] = "submitted"
    write_private(NAME, ledger)


def cleaned(p, actor, launch_id):
    ledger = load(p)
    for entry in ledger["entries"].values():
        if entry["actor"] == actor and entry.get("launch_id") == launch_id:
            entry["status"] = "cleaned"
    write_private(NAME, ledger)


def pending(p, actor):
    return [e for e in load(p)["entries"].values() if e["actor"] == actor and e["status"] in PENDING]
