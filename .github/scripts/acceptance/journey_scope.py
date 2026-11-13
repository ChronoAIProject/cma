"""Persisted visitor quota, exact Trigger pin, and owner-only cleanup scope."""

import launch_ledger
import trigger_pin


def allowed_write(p, tenant, path, method, body, key=None):
    if path.startswith("/api/v1/triggers/public/"):
        trigger = trigger_pin.configured(p)
        public_id = trigger.get("public_id")
        if (public_id and method == "POST"
                and path == "/api/v1/triggers/public/" + public_id + "/launches"):
            launch_ledger.validate(p, tenant, key, {
                "tenant": tenant, "path": path, "method": method, "body": body,
            })
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
            trigger_pin.launch(p, fixture)
            # All preparation starts only after the immutable policy is pinned.
            return True
    if len(parts) == 5 and parts[1:4] == ["api", "v1", "sandboxes"]:
        fixture = p.state["fixtures"].get("sandbox_" + tenant, {})
        if method == "DELETE" and body is None and parts[4] == fixture.get("id"):
            return fixture.get("owner") == p.accounts[tenant]["subject"]
    return False
