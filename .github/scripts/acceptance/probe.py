"""Owner-bound acceptance context; results contain no bodies or identifiers."""

import datetime
import hashlib
import json
import re
import uuid

from safety import CMA_ORIGIN, Http, NYX_ORIGIN, Refusal, decode, read_private, write_private


class Defect(Refusal):
    pass


class Probe:
    def __init__(self):
        self.http = Http()
        self.accounts = read_private("accounts.json")
        if not self.accounts or set(self.accounts) != {"a", "b"}:
            raise Refusal("two_test_accounts_required")
        if self.accounts["a"]["subject"] == self.accounts["b"]["subject"]:
            raise Refusal("distinct_test_accounts_required")
        self.state = read_private("probe-state.json", {
            "campaign": str(uuid.uuid4()), "fixtures": {}, "writes": {}, "results": [],
        })
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
                if isinstance(code, str) and re.fullmatch(r"[a-z_]{1,64}", code):
                    row["problem_code"] = code
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

    def login(self, tenant):
        account = self.accounts[tenant]
        response = self.http.request(NYX_ORIGIN, "/api/v1/auth/login", "POST", body={
            "email": account["email"], "password": account["password"], "client": "token",
        })
        if response[0] != 200:
            raise Refusal("test_account_login_unavailable")
        result = decode(response[2])
        if result.get("user_id") != account["subject"]:
            raise Refusal("test_account_subject_mismatch")
        account.update(access_token=result["access_token"], refresh_token=result.get("refresh_token"))
        write_private("accounts.json", self.accounts)

    def request(self, tenant, path, method="GET", body=None, headers=None, key=None):
        if tenant not in ("a", "b", None):
            raise Refusal("unapproved_actor")
        if method not in ("GET", "HEAD", "OPTIONS"):
            self.allow_write(tenant, path, method, body)
        fields = dict(headers or {})
        if key:
            fields["Idempotency-Key"] = key
        token = self.accounts[tenant]["access_token"] if tenant else None
        return self.http.request(CMA_ORIGIN, path, method, token, body, fields)

    def allow_write(self, tenant, path, method, body):
        if tenant not in ("a", "b"):
            raise Refusal("write_requires_test_actor")
        from journey_scope import allowed_write
        if allowed_write(self, tenant, path, method, body):
            return
        if (method, path) == ("POST", "/api/v1/documents"):
            return
        if method == "POST" and path == "/api/v1/usage/metrics/reports":
            filters = body.get("filters", {})
            for field in ("sandbox_id", "agent_id"):
                if filters.get(field) and filters[field] not in self.fixture_ids():
                    raise Refusal("unowned_usage_filter")
            if filters.get("owner") and filters["owner"] not in {v["subject"] for v in self.accounts.values()}:
                raise Refusal("unowned_usage_owner")
            return
        parts = path.split("/")
        if len(parts) >= 5 and parts[1:4] == ["api", "v1", "documents"]:
            if parts[4] not in self.fixture_ids():
                raise Refusal("write_target_outside_test_fixtures")
            if len(parts) == 5 and method == "PATCH":
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
        if entry["status"] == "complete":
            return entry["response_status"], {"Acceptance-Retained": "true"}, json.dumps(entry["response"]).encode(), 0.0
        # Persist the original operation before dispatch; an uncertain call reuses it.
        entry["status"] = "submitted"
        self.save()
        response = self.request(tenant, path, method, body, key=entry["key"])
        entry.update(status="complete", response_status=response[0],
                     response=decode(response[2]) if response[2] else None)
        self.save()
        return response

    def fixture(self, label, value):
        if value.get("owner_subject") and value["owner_subject"] not in {v["subject"] for v in self.accounts.values()}:
            raise Defect("SECURITY BUG: created_fixture_has_foreign_owner")
        self.state["fixtures"][label] = value
        self.save()
