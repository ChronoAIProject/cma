"""Run bounded acceptance phases; never print responses or credentials."""

import argparse
import datetime
import fcntl
import os
from pathlib import Path

import documents
import cleanup
import grant_cleanup
import retained
import launch_ledger
import domains
import journey
import oauth_session
import resources
import usage
from probe import Probe
import identity
import session_journal
from review_gate import require_go
from evidence import Evidence
from safety import CMA_ORIGIN, Refusal, decode, outside_quiet_window, private_root


def baseline(p):
    expected = {"create_sandbox", "use_own_sandboxes", "delete_own_sandbox", "launch_triggers"}
    for tenant in ("a", "b"):
        response = p.request(tenant, "/auth/session")
        session = decode(response[2])
        p.assert_case("identity_" + tenant, response[0] == 200 and session.get("authenticated") is True
                      and session.get("subject") == p.accounts[tenant]["subject"]
                      and set(session.get("capabilities", [])) == expected, [1549], response, True)
        response = p.request(tenant, "/api/v1/admin/settings")
        p.assert_case("spend_admin_read_denied_" + tenant, response[0] == 403
                      and decode(response[2]).get("code") == "forbidden", [1549, 1489], response, True)
        for name, route in (("sandbox", "/api/v1/sandboxes?limit=20"),
                            ("agent", "/api/v1/agents/provenance?limit=20")):
            response = p.request(tenant, route)
            p.assert_case(name + "_inventory_allowed_" + tenant, response[0] == 200, [1549], response)
            items = decode(response[2]).get("items", [])
            owned_ids = {value for label, fixture in p.state["fixtures"].items()
                         if label.endswith("_" + tenant)
                         for key, value in fixture.items()
                         if key in ("id", "agent_id", "sandbox_id") and isinstance(value, str)}
            p.assert_case(name + "_inventory_ownership_" + tenant,
                          all(x.get("owner_subject") == p.accounts[tenant]["subject"]
                              or x.get("id", x.get("agent_id")) in owned_ids for x in items),
                          [1549], security=True)
    for credential, label in ((None, "anonymous"), ("acceptance-invalid-credential", "invalid")):
        response = p.http.request(CMA_ORIGIN, "/api/v1/sandboxes?limit=1", token=credential)
        p.assert_case("authentication_" + label + "_denied", response[0] == 401
                      and decode(response[2]).get("code") == "not_authenticated", [1595, 1472], response, True)


def revoked_replay(p):
    # Reuse already-revoked tracked sessions; do not revoke control sessions yet.
    from safety import NYX_ORIGIN
    journal = session_journal.load(p)
    for actor in ("a", "b"):
        old = next((entry for entry in journal["tokens"].values()
                    if entry["actor"] == actor and entry["status"] == "revoked"), None)
        if not old:
            p.record("stale_session_replay_" + actor, "BLOCKED", [1472, 1549],
                     reason="no_previously_revoked_tracked_session; final_cleanup_runs_last")
            continue
        for label, origin, path in (("provider", NYX_ORIGIN, "/api/v1/users/me"),
                                    ("cma", CMA_ORIGIN, "/api/v1/sandboxes?limit=1")):
            response = p.http.request(origin, path, token=old["token"])
            p.assert_case("stale_session_" + label + "_" + actor, response[0] == 401,
                          [1472, 1549], response, True)


def full_suite(p, new=False):
    # Sprint #1472 controls precede domain, usage and broader journey evidence.
    for phase in (baseline, documents.run, documents.updates, revoked_replay, resources.run,
                  oauth_session.run, lambda context: journey.run(context, new=new),
                  usage.run, domains.run):
        identity.bind_live(p)
        phase(p)


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("phase", choices=("baseline", "documents", "updates", "sessions", "usage", "domains", "journey", "consent", "resources", "finish", "cleanup", "all"))
    parser.add_argument("--new-journey", action="store_true")
    parser.add_argument("--output", required=True, type=Path)
    args = parser.parse_args()
    if args.new_journey and args.phase not in ("journey", "all"):
        raise Refusal("new_journey_requires_journey_phase")
    outside_quiet_window()
    with Evidence(args.output) as evidence:
        return run_phase(args, evidence)


def run_phase(args, evidence):
    root = private_root()
    lock_fd = os.open(root / "runner.lock", os.O_CREAT | os.O_RDWR | os.O_NOFOLLOW, 0o600)
    with os.fdopen(lock_fd, "w") as lock:
        try:
            fcntl.flock(lock, fcntl.LOCK_EX | fcntl.LOCK_NB)
        except BlockingIOError:
            raise Refusal("acceptance_runner_already_active") from None
        if args.phase != "baseline":
            require_go()
        p = Probe()
        error = None
        ready = False
        try:
            identity.bind_live(p)
            p.http.writes_authorized = args.phase != "baseline"
            if args.phase != "baseline":
                # Read every durable boundary before enabling the first write.
                retained.load(p); grant_cleanup.load(p); launch_ledger.load(p)
                session_journal.register_current(p)
                ready = True
            if args.phase == "baseline":
                baseline(p)
            elif args.phase == "documents":
                documents.run(p)
            elif args.phase == "usage":
                usage.run(p)
            elif args.phase == "updates":
                documents.updates(p)
            elif args.phase == "domains":
                domains.run(p)
            elif args.phase == "journey":
                journey.run(p, new=args.new_journey)
            elif args.phase == "consent":
                oauth_session.run(p)
            elif args.phase == "resources":
                resources.run(p)
            elif args.phase in ("finish", "sessions", "cleanup"):
                cleanup.run(p)
            elif args.phase == "all":
                full_suite(p, new=args.new_journey)
        except KeyboardInterrupt:
            error = "acceptance_interrupted; cleanup_required"
        except Refusal as caught:
            error = str(caught)
        except Exception:
            error = "local_acceptance_harness_error; diagnostics_suppressed"
        finally:
            if ready and (error or args.phase == "all"):
                try:
                    cleanup.run(p)
                except Exception:
                    # Original error stays visible, but never leak cleanup payloads.
                    p.record("resumable_cleanup", "BLOCKED", [1549],
                             reason="cleanup_pending; resume_cleanup_with_approved_control_sessions")
                    error = error or "cleanup_pending"
            artifact = {"phase": args.phase, "started_at": p.started,
                        "finished_at": datetime.datetime.now(datetime.timezone.utc).isoformat(),
                        "requests": p.http.count, "results": p.results, "stop_reason": error}
            evidence.publish(artifact)
        if error:
            print(error, flush=True)
            return 1
        return 1 if any(row["result"] == "FAIL" for row in p.results) else 0


if __name__ == "__main__":
    try:
        raise SystemExit(main())
    except Refusal as error:
        print("BLOCKED: " + str(error))
        raise SystemExit(1)
    except Exception:
        print("BLOCKED: local acceptance harness error; no response or credential diagnostics emitted")
        raise SystemExit(1)
