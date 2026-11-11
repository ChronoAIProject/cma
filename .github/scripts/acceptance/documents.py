"""Small real document controls, tenant refusals, and revoked-receipt replay."""

from safety import decode

ISSUES = [1549, 1472, 1593]


def run(p):
    for owner, foreign in (("a", "b"), ("b", "a")):
        label = "document_" + owner
        if label not in p.state["fixtures"]:
            response = p.write(label + "_create", owner, "/api/v1/documents", "POST", {
                "title": "CMA automated acceptance " + owner,
                "markdown": "# Acceptance fixture\nSynthetic content only.\n",
            })
            if response[0] == 503:
                p.record(label + "_create", "BLOCKED", ISSUES, response, "document_storage_unavailable")
                continue
            p.assert_case(label + "_create", response[0] == 201, ISSUES, response)
            doc = decode(response[2])
            p.assert_case(label + "_owner", doc.get("owner_subject") == p.accounts[owner]["subject"], ISSUES, security=True)
            p.fixture(label, doc)
        doc = p.state["fixtures"][label]
        path = "/api/v1/documents/" + doc["id"]
        for suffix in ("", "/source", "/revisions", "/comments", "/shares", "/share-links"):
            case = label + (suffix.replace("/", "_") or "_metadata")
            response = p.request(foreign, path + suffix)
            problem = decode(response[2])
            p.assert_case(case + "_foreign_denied", response[0] == 404 and problem.get("code") == "document_not_found", ISSUES, response, True)
            response = p.request(owner, path + suffix)
            p.assert_case(case + "_owner_allowed", response[0] == 200, ISSUES, response)
        response = p.request(foreign, path, "PATCH", {
            "expected_revision": doc["revision"], "title": "Forbidden acceptance mutation",
        }, key=p.state["campaign"] + "-foreign-" + owner)
        p.assert_case(label + "_foreign_write_denied", response[0] == 404 and decode(response[2]).get("code") == "document_not_found", ISSUES, response, True)
        response = p.request(owner, path)
        current = decode(response[2])
        p.assert_case(label + "_foreign_write_unchanged", current.get("revision") == doc["revision"], ISSUES, response, True)
        for caller in (owner, foreign):
            response = p.request(caller, "/api/v1/documents?limit=20")
            items = decode(response[2]).get("items", [])
            present = any(row.get("id") == doc["id"] for row in items)
            p.assert_case(label + "_list_" + caller, response[0] == 200 and present == (caller == owner), ISSUES, response, True)
        if owner == "a":
            revoke_replay(p, path, doc, owner, foreign)


def revoke_replay(p, path, doc, owner, reader):
    share = path + "/shares/" + p.accounts[reader]["subject"]
    prior_grant = p.state["writes"].get("document_a_grant", {})
    grant_body = prior_grant.get("intent", {}).get("body", {
        "expected_revision": doc["revision"], "permission": "read"})
    revoke = p.state["writes"].get("document_a_revoke", {})
    if revoke.get("status") != "complete":
        response = p.write("document_a_grant", owner, share, "PUT", grant_body)
        p.assert_case("document_grant", response[0] == 200, ISSUES, response)
        granted = decode(response[2])
        response = p.request(reader, path + "/source")
        p.assert_case("document_shared_source_allowed", response[0] == 200, ISSUES, response)
        response = p.write("document_a_revoke", owner, share, "DELETE", {"expected_revision": granted["revision"]})
        p.assert_case("document_revoke", response[0] == 200, ISSUES, response)
        p.fixture("document_a", decode(response[2]))
    response = p.request(reader, path + "/source")
    p.assert_case("document_revoked_reader_denied", response[0] == 404 and decode(response[2]).get("code") == "document_not_found", ISSUES, response, True)
    original = p.state["writes"]["document_a_grant"]
    response = p.request(owner, share, "PUT", original["intent"]["body"], key=original["key"])
    p.assert_case("document_old_grant_receipt_replay", response[0] in (200, 409), ISSUES, response)
    response = p.request(reader, path + "/source")
    p.assert_case("document_replay_cannot_restore_access", response[0] == 404 and decode(response[2]).get("code") == "document_not_found", ISSUES, response, True)
    response = p.request(owner, path)
    p.assert_case("document_replay_preserves_revision", decode(response[2]).get("revision") == p.state["fixtures"]["document_a"]["revision"], ISSUES, response, True)


def updates(p):
    for owner in ("a", "b"):
        label = "document_" + owner
        if label not in p.state["fixtures"]:
            p.record(label + "_update", "BLOCKED", ISSUES, reason="owned_document_required")
            continue
        path = "/api/v1/documents/" + p.state["fixtures"][label]["id"]
        write_label = label + "_edit"
        if write_label in p.state["writes"]:
            body = p.state["writes"][write_label]["intent"]["body"]
        else:
            body = {"expected_revision": p.state["fixtures"][label]["revision"],
                    "title": "CMA acceptance edited " + owner, "markdown": "# Acceptance edit\nSynthetic update.\n"}
        response = p.write(write_label, owner, path, "PATCH", body)
        p.assert_case(label + "_owner_edit", response[0] == 200, ISSUES, response)
        current = decode(response[2])
        p.fixture(label, current)
        response = p.request(owner, path, "PATCH", body, key=p.state["campaign"] + "-stale-" + owner)
        p.assert_case(label + "_stale_revision_denied", response[0] == 409
                      and decode(response[2]).get("code") == "document_revision_conflict", ISSUES, response, True)
        response = p.request(owner, path + "/source")
        p.assert_case(label + "_edited_source_equal", response[0] == 200
                      and decode(response[2]).get("markdown") == body["markdown"], ISSUES, response)
