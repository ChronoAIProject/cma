"""A durable obligation precedes every test grant and stale grant replay."""

from identity import fingerprint, canonical_hash
import retained
from safety import Refusal, decode, read_private, write_private

NAME = "grant-cleanup.json"


def load(p):
    journal = read_private(NAME)
    subjects = {a: p.approval["actors"][a]["subject_sha256"] for a in ("a", "b")}
    if (not journal or journal.get("subjects") != subjects or journal.get("version") != 1
            or journal.get("complete_history") is not True):
        raise Refusal("grant_cleanup_history_missing_or_uncertain")
    return journal


def before_grant(p, actor, path, body, key):
    parts = path.split("/"); other = "b" if actor == "a" else "a"
    if len(parts) != 7 or parts[6] != p.accounts[other]["subject"]:
        raise Refusal("grant_must_target_other_approved_actor")
    retained.check_fixture(p, actor, "document", parts[4])
    journal = load(p)
    intent = {"tenant": actor, "path": path, "method": "PUT", "body": body}
    if not key or not any(w["key"] == key and w["intent"] == intent for w in p.state["writes"].values()):
        raise Refusal("grant_requires_original_durable_operation")
    journal["pending"][actor] = {"document_sha256": fingerprint(parts[4]), "reader": other,
                                "key_sha256": fingerprint(key), "intent_sha256": canonical_hash(intent)}
    write_private(NAME, journal)


def run(p):
    for actor, obligation in list(load(p)["pending"].items()):
        if actor not in ("a", "b") or obligation["reader"] != ("b" if actor == "a" else "a"):
            raise Refusal("grant_cleanup_actor_invalid")
        reader = obligation["reader"]
        doc = p.state["fixtures"].get("document_" + actor, {})
        if fingerprint(doc.get("id", "")) != obligation["document_sha256"]:
            raise Refusal("grant_cleanup_requires_original_document")
        retained.check_fixture(p, actor, "document", doc["id"])
        path = "/api/v1/documents/" + doc["id"]
        matches = [w for w in p.state["writes"].values()
                   if fingerprint(w["key"]) == obligation["key_sha256"]
                   and canonical_hash(w["intent"]) == obligation["intent_sha256"]]
        if len(matches) != 1:
            raise Refusal("grant_cleanup_requires_original_operation")
        original = matches[0]; intent = original["intent"]
        # A 404 read alone cannot settle an in-flight grant. Reconcile its exact
        # receipt first so a delayed accepted grant cannot revive after cleanup.
        response = p.request(actor, intent["path"], "PUT", intent["body"], key=original["key"])
        settled = response[0] == 200 or (response[0] == 409 and
                  decode(response[2]).get("code") == "document_revision_conflict")
        if not settled:
            raise Refusal("grant_original_receipt_still_uncertain")
        for _ in range(3):
            response = p.request(reader, path + "/source")
            if response[0] == 404 and decode(response[2]).get("code") == "document_not_found":
                journal = load(p); del journal["pending"][actor]; write_private(NAME, journal)
                p.record("document_grant_cleanup_" + actor, "PASS", [1549, 1593], response)
                break
            if response[0] != 200:
                raise Refusal("grant_cleanup_authority_unconfirmed")
            response = p.request(actor, path)
            current = decode(response[2])
            if response[0] != 200 or current.get("owner_subject") != p.accounts[actor]["subject"]:
                raise Refusal("grant_cleanup_owner_unconfirmed")
            p.fixture("document_" + actor, current)
            share = path + "/shares/" + p.accounts[reader]["subject"]
            label = "grant_cleanup_" + actor + "_" + str(current["revision"])
            response = p.write(label, actor, share, "DELETE", {"expected_revision": current["revision"]})
            if response[0] == 200:
                p.fixture("document_" + actor, decode(response[2]))
            elif response[0] != 409:
                raise Refusal("grant_cleanup_revoke_pending")
        else:
            raise Refusal("grant_cleanup_still_pending")
