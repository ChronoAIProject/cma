"""Owner-bound acceptance context; results contain no bodies or identifiers."""

import datetime
import hashlib
import json
import uuid

from safety import CMA_ORIGIN, Http, Refusal, decode, read_private, write_private
import identity
import launch_ledger
import retained
import grant_cleanup

# Protocol constants only. Never copy an arbitrary response string to evidence.
PROBLEM_CODES = frozenset({
    "not_authenticated", "forbidden", "not_found", "document_not_found",
    "document_revision_conflict", "invalid_query", "launch_not_found",
    "sandbox_not_found", "agent_not_found", "sandbox_credential_setup_required",
    "connections_required", "connection_unavailable", "resource_quota_exceeded",
    "model_budget_writes_unavailable", "model_request_budget_writes_unavailable",
})


class Defect(Refusal):
    pass


class Probe:
    def __init__(self):
        self.http = Http()
        self.accounts = read_private("accounts.json")
        self.approval = identity.approved()
        identity.bind_config(self.accounts or {}, self.approval)
        self.identities_bound = False
        self.state = read_private("probe-state.json")
        if not self.state:
            raise Refusal("missing_campaign_history_requires_reconciliation")
        self.results = []
        self.started = datetime.datetime.now(datetime.timezone.utc).isoformat()

    def save(self):
        write_private("probe-state.json", self.state)

    def record(self, case, result, issues, observation=None, reason=None):
        row = {"case": case, "result": result, "issues": issues}
        if observation:
            status, headers, body, elapsed = observation
            row.update(http_status=status, response_bytes=len(body),
                       response_sha256=hashlib.sha256(body).hexdigest(), seconds=elapsed)
            if headers.get("Acceptance-Retained") == "true":
                row["observation"] = "retained_write_receipt"
            try:
                code = json.loads(body).get("code")
                if code is not None:
                    row["problem_code"] = code if isinstance(code, str) and code in PROBLEM_CODES else "unrecognized"
            except (ValueError, AttributeError):
                pass
        if reason:
            row["reason"] = reason
        self.results.append(row)
        self.state["results"].append(row)
        self.save()
        print(json.dumps(row), flush=True)

    def assert_case(self, case, good, issues, observation=None, security=False):
        self.record(case, "PASS" if good else "FAIL", issues, observation)
        if not good:
            if security and observation and observation[0] >= 400:
                raise Refusal("PROBE CONTRACT MISMATCH: " + case)
            raise Defect(("SECURITY BUG: " if security else "BUG: ") + case)

    def request(self, tenant, path, method="GET", body=None, headers=None, key=None):
        if tenant not in ("a", "b", None):
            raise Refusal("unapproved_actor")
        if method not in ("GET", "HEAD", "OPTIONS"):
            self.allow_write(tenant, path, method, body, key)
            if method == "PUT" and "/documents/" in path and "/shares/" in path:
                grant_cleanup.before_grant(self, tenant, path, body, key)
        fields = dict(headers or {})
        if key:
            fields["Idempotency-Key"] = key
        token = self.accounts[tenant]["access_token"] if tenant else None
        return self.http.request(CMA_ORIGIN, path, method=method, token=token, body=body, headers=fields)

    def allow_write(self, tenant, path, method, body, key=None):
        if not self.identities_bound:
            raise Refusal("both_live_identities_required_before_write")
        if tenant not in ("a", "b"):
            raise Refusal("write_requires_test_actor")
        from journey_scope import allowed_write
        if allowed_write(self, tenant, path, method, body, key):
            return
        if (method, path) == ("POST", "/api/v1/documents"):
            retained.validate(self, tenant, key, {"tenant": tenant, "path": path, "method": method, "body": body})
            return
        if method == "POST" and path == "/api/v1/usage/metrics/reports":
            retained.validate(self, tenant, key, {"tenant": tenant, "path": path, "method": method, "body": body})
            filters = body.get("filters", {})
            for field in ("sandbox_id", "agent_id"):
                if filters.get(field) and filters[field] not in self.fixture_ids():
                    raise Refusal("unowned_usage_filter")
            if filters.get("owner") and filters["owner"] not in {v["subject"] for v in self.accounts.values()}:
                raise Refusal("unowned_usage_owner")
            return
        parts = path.split("/")
        if len(parts) >= 5 and parts[1:4] == ["api", "v1", "documents"]:
            owners = [a for a in ("a", "b") if self.state["fixtures"].get("document_" + a, {}).get("id") == parts[4]]
            if len(owners) != 1:
                raise Refusal("write_target_outside_test_fixtures")
            owner = owners[0]
            retained.check_fixture(self, owner, "document", parts[4])
            if len(parts) == 5 and method == "PATCH":
                if retained.source_slot_consumed(self, owner):
                    if tenant != owner:
                        raise Refusal("consumed_document_slot_forbids_dispatch")
                    retained.validate(self, tenant, key, {"tenant": tenant, "path": path, "method": method, "body": body})
                if tenant == owner:
                    doc = self.state["fixtures"]["document_" + owner]
                    if not isinstance(body.get("expected_revision"), int):
                        raise Refusal("document_revision_required")
                    if body["expected_revision"] >= doc["revision"]:
                        retained.validate(self, tenant, key, {"tenant": tenant, "path": path, "method": method, "body": body})
                return
            if len(parts) == 7 and parts[5] == "shares" and method in ("PUT", "DELETE"):
                if parts[6] in {v["subject"] for v in self.accounts.values()}:
                    return
        raise Refusal("write_route_not_allowed")

    def fixture_ids(self):
        return {value for f in self.state["fixtures"].values() for key, value in f.items()
                if key in ("id", "agent_id", "sandbox_id", "workspace_id", "report_id") and isinstance(value, str)}

    def write(self, label, tenant, path, method, body):
        writes = self.state["writes"]
        intent = {"tenant": tenant, "path": path, "method": method, "body": body}
        if label not in writes:
            writes[label] = {"intent": intent, "key": str(uuid.uuid4()), "status": "prepared"}
            self.save()
        entry = writes[label]
        if entry["intent"] != intent:
            raise Refusal("idempotency_body_changed")
        is_launch = method == "POST" and path.startswith("/api/v1/triggers/public/") and path.endswith("/launches")
        if is_launch:
            if entry["key"] not in launch_ledger.load(self)["entries"]:
                # A crash may have saved the campaign intent before reservation.
                # That is still a fresh admission and needs current server proof.
                from launch_recovery import prove_history_and_empty_inventory
                prove_history_and_empty_inventory(self, tenant)
            launch_ledger.reserve(self, tenant, entry["key"], intent)
        retained.reserve(self, tenant, entry["key"], intent)
        if entry["status"] == "complete":
            retained.observed(self, tenant, entry["key"], intent, entry["response_status"], entry["response"])
            if is_launch:
                launch_ledger.observed(self, entry["key"], entry["response_status"], entry["response"])
            return entry["response_status"], {"Acceptance-Retained": "true"}, json.dumps(entry["response"]).encode(), 0.0
        # Persist the original operation before dispatch; an uncertain call reuses it.
        entry["status"] = "submitted"
        self.save()
        if is_launch:
            launch_ledger.submitted(self, entry["key"])
        response = self.request(tenant, path, method, body, key=entry["key"])
        payload = decode(response[2]) if response[2] else None
        uncertain = response[0] >= 500 or (isinstance(payload, dict) and payload.get("code") in {
            "idempotency_in_flight", "idempotency_recovery_required"})
        entry.update(status="submitted" if uncertain else "complete", response_status=response[0],
                     response=payload)
        self.save()
        if is_launch:
            launch_ledger.observed(self, entry["key"], response[0], entry["response"])
        retained.observed(self, tenant, entry["key"], intent, response[0], entry["response"])
        return response

    def fixture(self, label, value):
        if value.get("owner_subject") and value["owner_subject"] not in {v["subject"] for v in self.accounts.values()}:
            raise Defect("SECURITY BUG: created_fixture_has_foreign_owner")
        self.state["fixtures"][label] = value
        self.save()
