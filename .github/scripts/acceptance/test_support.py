"""Synthetic actors and private journals; no production fixtures or network."""

from contextlib import contextmanager
from pathlib import Path
import tempfile
from unittest.mock import patch

from identity import canonical_hash, fingerprint
from probe import Probe
from safety import Http, write_private


def ordinary(subject):
    return {"id": subject, "is_admin": False, "is_operator": False, "role": "user",
            "is_active": True, "capabilities": {key: False for key in (
                "manage_credit_grants", "manage_catalog", "manage_tools", "read_admin_usage",
                "read_feature_flags", "create_feature_flags", "manage_all_feature_flags")}}


@contextmanager
def context():
    with tempfile.TemporaryDirectory() as temporary, patch("safety.SECRET_ROOT", Path(temporary).resolve() / "private"):
        p = object.__new__(Probe)
        p.accounts = {a: {"subject": "test-" + a, "email": a + "@example.invalid", "access_token": "bearer-" + a,
                          "cma_client_id": "test-client"} for a in ("a", "b")}
        descriptor = {"availability": "published", "always_review": True, "first_message": "",
                      "trust": {"disclosures": []},
                      "technical": {"provider": "codex", "effort": "minimal", "services": ["chrono-llm-public"]}}
        p.approval = {"actors": {a: {"subject_sha256": fingerprint(v["subject"]), "email_sha256": fingerprint(v["email"])}
                                  for a, v in p.accounts.items()}, "capabilities": ["launch_triggers"],
                      "cma_client_sha256": fingerprint("test-client"),
                      "trigger": {"name": "nightly-smoke-readonly", "public_id_sha256": fingerprint("test-trigger"),
                                  "descriptor_sha256": canonical_hash(descriptor), "revision_digest": "test-revision",
                                  "policy": dict(descriptor["technical"], always_review=True, first_message="", disclosures=[])},
                      "limits": {"launch_attempts_per_actor_sgt_day": 5}}
        p.state = {"campaign": "synthetic-campaign", "fixtures": {}, "writes": {}, "results": []}
        p.results = []; p.identities_bound = True; p.started = "synthetic-start"
        p.http = Http(); p.http.writes_authorized = True
        p.http.request = lambda *a, **kw: (_ for _ in ()).throw(AssertionError("unmocked_network_forbidden"))
        p.save = lambda: write_private("probe-state.json", p.state)
        p.save()
        subjects = {a: v["subject_sha256"] for a, v in p.approval["actors"].items()}
        for name, field in (("launch-ledger.json", "entries"), ("session-journal.json", "tokens"),
                            ("retained-fixtures.json", "slots"), ("grant-cleanup.json", "pending")):
            write_private(name, {"version": 1, "complete_history": True, "subjects": subjects, field: {}})
        write_private("acceptance-trigger.json", {"public_id": "test-trigger", "public_descriptor": descriptor})
        yield p


def restart(p):
    from safety import read_private
    p.state = read_private("probe-state.json")
    p.results = []
    return p


def launch_value(sandbox=False):
    return {"id": "test-launch", "public_id": "test-trigger", "revision_digest": "test-revision",
            "state": "draft", "trust": {"disclosures": []},
            "resolution": {"sandbox": {"id": "test-sandbox"}} if sandbox else {}}


def intent(actor="a"):
    return {"tenant": actor, "path": "/api/v1/triggers/public/test-trigger/launches",
            "method": "POST", "body": {"start_if_ready": False}}
