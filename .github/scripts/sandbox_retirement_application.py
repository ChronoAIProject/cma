"""Read and submit retirement through the mandatory application guard."""

from hashlib import sha256
import json
from time import monotonic, sleep
from urllib.error import HTTPError
from urllib.parse import quote
from urllib.request import HTTPRedirectHandler, ProxyHandler, Request, build_opener

from sandbox_retirement_execution import Admission, Eligibility, Observation
from sandbox_retirement_finality import Finality, evaluate, matches_preimage


MAX_RESPONSE = 4 * 1024 * 1024


class NoRedirect(HTTPRedirectHandler):
    def redirect_request(self, request, stream, status, reason, headers, url):
        return None


def unique_fields(pairs):
    values = dict(pairs)
    if len(values) != len(pairs):
        raise ValueError("duplicate response field")
    return values


class JsonTransport:
    def __init__(self, origin, token):
        self.origin = origin
        self.client = build_opener(ProxyHandler({}), NoRedirect())
        self.headers = {
            "Authorization": f"Bearer {token}", "Accept": "application/json", "Cache-Control": "no-cache",
        }

    def read(self, path, timeout):
        return self.request(path, timeout)

    def submit(self, path, body, timeout, before_send):
        return self.request(path, timeout, json.dumps(body, separators=(",", ":")).encode(), before_send)

    def request(self, path, timeout, data=None, before_send=None):
        headers = self.headers
        if data is not None:
            headers = {**headers, "Content-Type": "application/json"}
        request = Request(self.origin + path, data=data, headers=headers)
        if before_send is not None and not before_send():
            return None
        try:
            with self.client.open(request, timeout=min(timeout, 5)) as response:
                data = response.read(MAX_RESPONSE + 1)
                if response.status != 200 or len(data) > MAX_RESPONSE:
                    raise ValueError("unexpected response")
                return response.status, json.loads(data, object_pairs_hook=unique_fields)
        except HTTPError as error:
            with error:
                if error.code != 404:
                    return error.code, None
                data = error.read(MAX_RESPONSE + 1)
                if len(data) > MAX_RESPONSE:
                    return error.code, None
                try:
                    return error.code, json.loads(data, object_pairs_hook=unique_fields)
                except ValueError:
                    return error.code, None


class ProviderReads:
    def __init__(self, reader):
        self.reader = reader

    def read(self, expected, deadline):
        namespace = quote(expected["namespace"], safe="")
        core = f"/api/v1/namespaces/{namespace}"
        sandbox = f"/apis/agents.x-k8s.io/v1beta1/namespaces/{namespace}/sandboxes"
        claim = f"/apis/extensions.agents.x-k8s.io/v1beta1/namespaces/{namespace}/sandboxclaims"
        paths = {
            "sandbox": sandbox, "claim": claim,
            "pvc": core + "/persistentvolumeclaims", "pv": "/api/v1/persistentvolumes",
        }
        observed = {}
        groups = {"sandbox": "agents.x-k8s.io", "claim": "extensions.agents.x-k8s.io"}
        types = {
            "sandbox": "Sandbox", "claim": "SandboxClaim", "pvc": "PersistentVolumeClaim",
            "pv": "PersistentVolume",
        }
        for kind, prefix in paths.items():
            name = expected[kind]["name"]
            path = prefix + "/" + quote(name, safe="")
            status, value = self.get(path, deadline)
            if (
                status == 404 and isinstance(value, dict)
                and value.get("kind") == "Status" and value.get("reason") == "NotFound"
                and value.get("code") == 404 and value["details"]["name"] == name
                and value["details"].get("group", "") == groups.get(kind, "")
                and value["details"]["kind"] == prefix.rsplit("/", 1)[1]
            ):
                observed[kind] = None
            elif status == 200 and isinstance(value, dict) and value.get("kind") == types[kind]:
                if kind != "pv" and value["metadata"].get("namespace") != expected["namespace"]:
                    raise ValueError("provider object namespace changed")
                observed[kind] = value
            else:
                raise ValueError("provider object read unavailable")
        status, pod_list = self.get(core + "/pods", deadline)
        if status != 200 or pod_list["kind"] != "PodList" or pod_list["metadata"].get("continue"):
            raise ValueError("complete provider Pod list unavailable")
        observed["pods"] = pod_list["items"]
        return observed

    def get(self, path, deadline):
        remaining = deadline - monotonic()
        if remaining <= 0:
            raise TimeoutError("provider observation deadline")
        return self.reader.read(path, remaining)


def digest(value, *, ordered=False):
    encoded = json.dumps(
        value, ensure_ascii=not ordered, sort_keys=not ordered, separators=(",", ":"), allow_nan=False,
    ).encode()
    if ordered:
        encoded = b"cma-reviewed-retirement-v1\0" + encoded
    return sha256(encoded).hexdigest()


def same_sandbox(member, sandbox):
    return all(sandbox[key] == member[key] for key in ("id", "owner", "created_at"))


def retained_storage_matches(member, home):
    provider = member["provider"]
    receipt = home["receipt"]
    origin = receipt["native_authority"]
    claim = origin["claim"]
    return (
        home["request"]["sandbox_id"] == member["id"]
        and claim["name"] == provider["claim"]["name"]
        and claim["uid"] == provider["claim"]["uid"]
        and claim["sandbox_name"] == provider["sandbox"]["name"]
        and origin["sandbox_uid"] == provider["sandbox"]["uid"]
        and origin["pvc_uid"] == provider["pvc"]["uid"]
        and receipt["storage_generation"] == provider["pvc"]["uid"]
        and receipt["storage_reference"] == provider["pvc"]["name"]
    )


def make_guard(batch, member, sandbox):
    # Match the Rust DTO's field order. The nested typed objects come directly
    # from the application's serde JSON representation.
    return {
        "batch_sha256": batch,
        "member_sha256": member["member_sha256"],
        "sandbox_id": member["id"],
        "owner": member["owner"],
        "created_at": member["created_at"],
        "revision": member["revision"],
        "status": sandbox["status"],
        "last_activity_at": sandbox["last_activity_at"],
        "idle_before": member["idle_before"],
        "runtime": sandbox["runtime"],
        "home": sandbox["lifecycle"]["home"],
    }


class ApplicationRetirement:
    def __init__(self, app, provider, batch, cutoff, inspect, timeout):
        self.app = app
        self.provider = provider
        self.batch = batch
        self.cutoff = cutoff
        self.inspect = inspect
        self.timeout = timeout
        self.prepared = None

    def path(self, member):
        return f"/api/v1/admin/sandboxes/{member['id']}"

    def preflight(self, member):
        self.prepared = None
        try:
            deadline = monotonic() + self.timeout
            status, detail = self.app.read(self.path(member), self.timeout)
            if status != 200:
                return Eligibility.UNKNOWN, None
            sandbox = detail["sandbox"]
            disposition = self.inspect(member["snapshot"], detail, self.cutoff)
            if disposition == "observe_existing_retirement":
                return Eligibility.EXISTING, None
            if disposition in {"recent_or_active_work", "snapshot_changed"}:
                return Eligibility.SKIP, digest({"disposition": disposition, "revision": sandbox["revision"]})
            if disposition != "eligible_for_review":
                return Eligibility.UNKNOWN, None
            home = sandbox["lifecycle"]["home"]
            if not retained_storage_matches(member, home):
                return Eligibility.UNKNOWN, None
            provider = self.provider.read(member["provider"], deadline)
            if not matches_preimage(member["provider"], provider) or monotonic() >= deadline:
                return Eligibility.UNKNOWN, None
            guard = make_guard(self.batch, member, sandbox)
            fingerprint = digest(guard, ordered=True)
            self.prepared = guard, fingerprint
            return Eligibility.ELIGIBLE, fingerprint
        except (KeyError, TypeError, ValueError, OSError):
            return Eligibility.UNKNOWN, None

    def dispatch(self, member, before_send):
        guard, fingerprint = self.prepared
        self.prepared = None
        if guard["member_sha256"] != member["member_sha256"]:
            raise ValueError("prepared guard does not match the ordered member")
        try:
            response = self.app.submit(self.path(member) + "/retire-reviewed", guard, self.timeout, before_send)
            if response is None:
                return Admission.UNSENT, None
            status, reply = response
            if status != 200:
                return Admission.UNKNOWN, None
            outcome = reply["status"]
            if outcome == "accepted" and self.matches_receipt(member, fingerprint, reply["receipt"]):
                return Admission.ACCEPTED, None
            if outcome == "observe_existing":
                return Admission.EXISTING, None
            if outcome == "not_eligible":
                return Admission.REFUSED, digest(reply)
        except (KeyError, TypeError, ValueError, OSError):
            pass
        return Admission.UNKNOWN, None

    def matches_receipt(self, member, fingerprint, receipt):
        return (
            receipt["batch_sha256"] == self.batch
            and receipt["member_sha256"] == member["member_sha256"]
            and receipt["sandbox_id"] == member["id"]
            and receipt["guard_sha256"] == fingerprint
            and receipt["accepted_revision"] == member["revision"] + 1
            and type(receipt["event_sequence"]) is int
            and receipt["event_sequence"] > 0
        )

    def observe(self, member, *, guard_sha256):
        deadline = monotonic() + self.timeout
        path = self.path(member)
        while (remaining := deadline - monotonic()) > 0:
            try:
                status, detail = self.app.read(path, remaining)
                if status == 200:
                    sandbox = detail["sandbox"]
                    if not same_sandbox(member, sandbox):
                        return None
                    if sandbox["status"] == "retired":
                        home = sandbox["lifecycle"]["home"]
                        if (
                            sandbox.get("runtime") is not None or home["released"] is not True
                            or not retained_storage_matches(member, home)
                        ):
                            return None
                        receipt = self.read_receipt(member, guard_sha256, deadline)
                        if guard_sha256 is not None and receipt is None:
                            return None
                        if receipt is not None and sandbox["revision"] <= receipt["accepted_revision"]:
                            return None
                        observed = self.provider.read(member["provider"], deadline)
                        finality = evaluate(member["provider"], observed)
                        if finality in {Finality.INVALID, Finality.REPLACED}:
                            return None
                        if finality is Finality.SETTLED and monotonic() < deadline:
                            return Observation(digest({
                                "revision": sandbox["revision"], "receipt": receipt,
                                "disposition": member["provider"]["storage"],
                                "provider_evidence_sha256": digest(observed),
                            }))
            except (KeyError, TypeError, ValueError, OSError):
                return None
            sleep(max(0, min(2, deadline - monotonic())))
        return None

    def read_receipt(self, member, fingerprint, deadline):
        if fingerprint is None:
            return None
        remaining = deadline - monotonic()
        if remaining <= 0:
            return None
        path = f"{self.path(member)}/retire-reviewed/{self.batch}/{member['member_sha256']}"
        status, receipt = self.app.read(path, remaining)
        if status == 200 and self.matches_receipt(member, fingerprint, receipt):
            return receipt
        return None
