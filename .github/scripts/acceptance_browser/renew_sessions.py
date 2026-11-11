"""Lead #215: renew only the approved ordinary accounts, never provision users."""

import argparse
import fcntl
import json
import os
from pathlib import Path
import sys

sys.path.insert(0, str(Path(__file__).resolve().parent.parent / "acceptance"))
from evidence import Evidence
from identity import bind_config, bind_live, fingerprint
import oauth_session
from probe import Probe
import session_journal
from safety import (
    NYX_ORIGIN,
    Refusal,
    decode,
    outside_quiet_window,
    private_root,
    read_private,
    write_private,
)

JOURNAL = "web-verification-renewal-20261112.json"


def renew(p, report):
    """A submitted login is never retried without its original durable receipt."""
    bind_config(p.accounts, p.approval)
    pins = {a: p.approval["actors"][a]["subject_sha256"] for a in ("a", "b")}
    journal = read_private(JOURNAL, {"version": 1, "subjects": pins, "actors": {}})
    if journal.get("version") != 1 or journal.get("subjects") != pins:
        raise Refusal("renewal_journal_binding_mismatch")
    p.http.writes_authorized = True
    for actor in ("a", "b"):
        entry = journal["actors"].get(actor)
        if entry and entry.get("status") != "received":
            raise Refusal("prior_login_requires_reconciliation")
        if not entry:
            account = p.accounts[actor]
            if not account.get("password"):
                raise Refusal("dedicated_password_missing")
            journal["actors"][actor] = {"status": "submitted"}
            write_private(JOURNAL, journal)
            response = p.http.request(
                NYX_ORIGIN,
                "/api/v1/auth/login",
                "POST",
                body={
                    "email": account["email"],
                    "password": account["password"],
                    "client": "token",
                },
            )
            payload = decode(response[2])
            report["login_statuses"].append(response[0])
            if response[0] != 200:
                code = payload.get("error")
                reason = {
                    "mfa_required": "interactive_mfa_required",
                    "email_verification_required": "interactive_email_verification_required",
                    "captcha_required": "interactive_captcha_required",
                }.get(code, "ordinary_login_refused")
                if payload.get("error_code") == 2002:
                    reason = "interactive_mfa_required"
                journal["actors"][actor].update(status="refused", reason=reason)
                write_private(JOURNAL, journal)
                raise Refusal(reason)
            # Tokens go directly to the approved private store, including uncertain
            # identity responses. They never enter evidence, stdout or process argv.
            journal["actors"][actor] = {"status": "received", "receipt": payload}
            write_private(JOURNAL, journal)
            entry = journal["actors"][actor]
        receipt = entry["receipt"]
        if (
            fingerprint(receipt.get("user_id", "")) != pins[actor]
            or not receipt.get("access_token")
            or not receipt.get("refresh_token")
        ):
            raise Refusal("login_identity_or_token_contract_mismatch")
        p.accounts[actor].update(
            access_token=receipt["access_token"],
            refresh_token=receipt["refresh_token"],
            session_state="active",
        )
        write_private("accounts.json", p.accounts)
    bind_live(p)
    session_journal.register_current(p)
    report["ordinary_bearers_bound"] = 2
    journal["bearers_bound"] = True
    write_private(JOURNAL, journal)


def run(output):
    outside_quiet_window()
    report = {
        "task": "1667_dedicated_session_renewal",
        "login_statuses": [],
        "ordinary_bearers_bound": 0,
        "ordinary_cookies_bound": 0,
    }
    with Evidence(output) as evidence:
        root = private_root()
        fd = os.open(
            root / "runner.lock", os.O_CREAT | os.O_RDWR | os.O_NOFOLLOW, 0o600
        )
        with os.fdopen(fd, "w") as lock:
            fcntl.flock(lock, fcntl.LOCK_EX | fcntl.LOCK_NB)
            p = Probe()
            try:
                renew(p, report)
                journal = read_private(JOURNAL)
                if not journal.get("cookies_bound"):
                    # This is the existing reviewed normal-CMA consent flow only.
                    oauth_session.run(p)
                from binding import bind_browser

                bind_browser(p)
                report["ordinary_cookies_bound"] = 2
                journal["cookies_bound"] = True
                write_private(JOURNAL, journal)
                report["status"] = "PASS"
            except Refusal as error:
                allowed = {
                    "interactive_mfa_required",
                    "interactive_email_verification_required",
                    "interactive_captcha_required",
                    "ordinary_login_refused",
                    "prior_login_requires_reconciliation",
                    "dedicated_password_missing",
                    "login_identity_or_token_contract_mismatch",
                    "fresh_ordinary_test_session_required",
                    "prior_cma_consent_attempt_requires_cleanup",
                    "fresh_dedicated_cookie_required",
                }
                reason = (
                    str(error)
                    if str(error) in allowed
                    else "renewal_or_binding_refused"
                )
                report.update(status="BLOCKED", reason=reason)
            evidence.publish(report)
            print(json.dumps(report))
            return 0 if report["status"] == "PASS" else 1


if __name__ == "__main__":
    parser = argparse.ArgumentParser()
    parser.add_argument("--output", required=True)
    args = parser.parse_args()
    try:
        sys.exit(run(args.output))
    except Exception:
        print(
            "RENEWAL BLOCKED: private preflight or transport did not complete; reconcile original journal"
        )
        sys.exit(1)
