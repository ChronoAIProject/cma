"""Owner and foreign resource checks, only on resources of approved Launches."""

from safety import decode


def probe_resource(p, owner, foreign, kind, resource_id):
    collection = {"agent": "agents", "sandbox": "sandboxes"}[kind]
    path = "/api/v1/" + collection + "/" + resource_id
    label = kind + "_" + owner
    issues = [1549, 1594] if kind == "agent" else [1549]
    suffixes = ("", "/events?after=0&limit=10")
    if kind == "agent":
        suffixes += ("/transcript", "/history", "/usage")
    for index, suffix in enumerate(suffixes):
        response = p.request(foreign, path + suffix)
        p.assert_case(label + "_foreign_read_" + str(index), response[0] == 404
                      and decode(response[2]).get("code") == kind + "_not_found",
                      issues, response, True)
        response = p.request(owner, path + suffix)
        if response[0] == 503 and suffix == "/history":
            p.record(label + "_owner_history", "BLOCKED", issues, response,
                     "owned_runtime_export_unavailable")
        else:
            p.assert_case(label + "_owner_read_" + str(index), response[0] == 200, issues, response)
    for caller in (owner, foreign):
        response = p.request(caller, "/api/v1/" + collection + "?limit=20")
        present = any(item.get("id") == resource_id for item in decode(response[2]).get("items", []))
        p.assert_case(label + "_inventory_" + caller, response[0] == 200 and present == (caller == owner),
                      issues, response, True)
    before = p.request(owner, path)
    revision = decode(before[2]).get("revision")
    response = p.write(label + "_foreign_delete", foreign, path, "DELETE", None)
    p.assert_case(label + "_foreign_delete_denied", response[0] == 404
                  and decode(response[2]).get("code") == kind + "_not_found", issues, response, True)
    after = p.request(owner, path)
    p.assert_case(label + "_foreign_write_unchanged", after[0] == 200
                  and revision == decode(after[2]).get("revision"), issues, after, True)


def run(p):
    for owner, foreign in (("a", "b"), ("b", "a")):
        run_tenant(p, owner, foreign)


def run_tenant(p, owner, foreign):
    launch = p.state["fixtures"].get("launch_" + owner, {})
    for kind in ("sandbox", "agent"):
        resource_id = launch.get("resolution", {}).get(kind, {}).get("id")
        if not resource_id or p.state.get("cleaned_sandboxes", {}).get(owner):
            p.record(kind + "_production_isolation_" + owner, "BLOCKED", [1549, 1594, 1596],
                     reason="approved_visitor_launch_has_no_live_owned_resource; no_unrelated_fixture_used")
            continue
        # The resolution came from this test account's owned Launch.
        p.fixture(kind + "_" + owner, {"id": resource_id, "owner": p.accounts[owner]["subject"]})
        probe_resource(p, owner, foreign, kind, resource_id)
