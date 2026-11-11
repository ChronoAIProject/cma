"""Run bounded acceptance phases; never print responses or credentials."""

import argparse
import datetime
import fcntl
import json
import os
from pathlib import Path

import documents
import domains
import journey
import oauth_session
import resources
import finish
import sessions
import usage
from probe import Defect, Probe
from safety import CMA_ORIGIN, Refusal, decode, outside_quiet_window, private_root


def baseline(p):
    expected = {"create_sandbox", "use_own_sandboxes", "delete_own_sandbox", "launch_triggers"}
    for tenant in ("a", "b"):
        p.login(tenant)
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


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("phase", choices=("baseline", "documents", "updates", "sessions", "usage", "domains", "journey", "consent", "resources", "finish"))
    parser.add_argument("--output", required=True, type=Path)
    args = parser.parse_args()
    outside_quiet_window()
    root = private_root()
    lock_fd = os.open(root / "runner.lock", os.O_CREAT | os.O_RDWR, 0o600)
    with os.fdopen(lock_fd, "w") as lock:
        try:
            fcntl.flock(lock, fcntl.LOCK_EX | fcntl.LOCK_NB)
        except BlockingIOError:
            raise Refusal("acceptance_runner_already_active") from None
        p = Probe()
        error = None
        try:
            if args.phase == "baseline":
                baseline(p)
            elif args.phase == "documents":
                documents.run(p)
            elif args.phase == "usage":
                usage.run(p)
            elif args.phase == "updates":
                documents.updates(p)
            elif args.phase == "sessions":
                sessions.run(p)
            elif args.phase == "domains":
                domains.run(p)
            elif args.phase == "journey":
                journey.run(p)
            elif args.phase == "consent":
                oauth_session.run(p)
            elif args.phase == "resources":
                resources.run(p)
            elif args.phase == "finish":
                finish.run(p)
        except Refusal as caught:
            error = str(caught)
        except Exception:
            error = "local_acceptance_harness_error; diagnostics_suppressed"
        finally:
            artifact = {"phase": args.phase, "started_at": p.started,
                        "finished_at": datetime.datetime.now(datetime.timezone.utc).isoformat(),
                        "requests": p.http.count, "results": p.results, "stop_reason": error}
            if args.output.is_symlink():
                raise Refusal("evidence_symlink")
            fd = os.open(args.output, os.O_CREAT | os.O_EXCL | os.O_WRONLY, 0o600)
            with os.fdopen(fd, "w") as out:
                json.dump(artifact, out, indent=2)
                out.write("\n")
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
