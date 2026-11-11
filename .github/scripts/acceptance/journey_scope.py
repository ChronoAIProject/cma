"""Persisted visitor quota, exact Trigger pin, and owner-only cleanup scope."""

import datetime

from safety import Refusal, read_private


def allowed_write(p, tenant, path, method, body):
    if path.startswith("/api/v1/triggers/public/"):
        trigger = read_private("acceptance-trigger.json", {})
        public_id = trigger.get("public_id")
        if (public_id and method == "POST"
                and path == "/api/v1/triggers/public/" + public_id + "/launches"):
            if body != {"start_if_ready": True}:
                raise Refusal("unapproved_launch_intent")
            if not p.state.get("launch_reservations", {}).get(tenant):
                raise Refusal("launch_requires_durable_quota_reservation")
            return True
    parts = path.split("/")
    if len(parts) == 5 and parts[1:3] == ["api", "v1"] and parts[3] in ("agents", "sandboxes"):
        foreign = "b" if tenant == "a" else "a"
        kind = {"agents": "agent", "sandboxes": "sandbox"}[parts[3]]
        fixture = p.state["fixtures"].get(kind + "_" + foreign, {})
        if (method == "DELETE" and body is None and parts[4] == fixture.get("id")
                and fixture.get("owner") == p.accounts[foreign]["subject"]):
            return True  # Negative write, scoped strictly to the other TEST account.
    if len(parts) == 6 and parts[1:4] == ["api", "v1", "launches"]:
        fixture = p.state["fixtures"].get("launch_" + tenant, {})
        if parts[4] != fixture.get("id") or method != "POST":
            return False
        if parts[5] == "cancel" and body is None:
            return True
        if parts[5] == "start" and body == {"acknowledgements": []}:
            # No software trust, permissions, connection, or policy changes.
            return True
    if len(parts) == 5 and parts[1:4] == ["api", "v1", "sandboxes"]:
        fixture = p.state["fixtures"].get("sandbox_" + tenant, {})
        if method == "DELETE" and body is None and parts[4] == fixture.get("id"):
            return fixture.get("owner") == p.accounts[tenant]["subject"]
    return False


def reserve_launch(p, tenant, now=None):
    """One campaign Launch per visitor; reservation survives uncertain dispatch."""
    now = now or datetime.datetime.now(datetime.timezone.utc)
    day = now.astimezone(datetime.timezone(datetime.timedelta(hours=8))).date().isoformat()
    reservations = p.state.setdefault("launch_reservations", {})
    if tenant in reservations:
        if reservations[tenant]["day"] != day:
            raise Refusal("prior_day_launch_requires_explicit_new_campaign")
        return
    ledger = p.state.setdefault("launch_daily_counts", {})
    key = day + ":" + tenant
    if ledger.get(key, 0) >= 5:
        raise Refusal("visitor_daily_launch_budget")
    if "sandbox_" + tenant in p.state["fixtures"]:
        raise Refusal("one_visitor_sandbox_limit")
    ledger[key] = ledger.get(key, 0) + 1
    reservations[tenant] = {"day": day}
    p.save()
