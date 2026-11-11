"""Create ordinary acceptance users through NyxID's documented admin API."""

import argparse
import json
import os
from pathlib import Path
import secrets

from safety import Http, NYX_ORIGIN, Refusal, decode, read_private, write_private


def bootstrap(manifest):
    saved = Path.home() / ".nyxid"
    profile = os.environ.get("NYXID_PROFILE")
    if profile:
        if not all(ch.isalnum() or ch in "_-" for ch in profile):
            raise Refusal("invalid_cli_profile")
        saved = saved / "profiles" / profile
    origin = os.environ.get("NYXID_URL") or (saved / "base_url").read_text().strip()
    if origin != NYX_ORIGIN:
        raise Refusal("unexpected_identity_origin")
    admin_token = os.environ.get("NYXID_ACCESS_TOKEN") or (saved / "access_token").read_text().strip()
    http = Http()
    accounts = read_private("accounts.json", {})
    summary = []
    for alias in ("a", "b"):
        if alias not in accounts:
            accounts[alias] = {
                "email": "cma-accept-" + alias + "-" + secrets.token_hex(5) + "@acceptance.invalid",
                "display_name": "cma-accept-" + alias + " (Automated acceptance)",
                "password": secrets.token_urlsafe(36),
                "creation_state": "prepared",
            }
            write_private("accounts.json", accounts)
        account = accounts[alias]
        if account["creation_state"] == "prepared":
            account["creation_state"] = "submitted"
            write_private("accounts.json", accounts)
            code, _, payload, _ = http.request(NYX_ORIGIN, "/api/v1/admin/users", "POST", admin_token, {
                "email": account["email"], "password": account["password"],
                "display_name": account["display_name"], "role": "user",
            })
            if code != 200:
                raise Refusal("account_creation_http_" + str(code))
            result = decode(payload)
            if result.get("is_admin") or result.get("is_operator") or not result.get("is_active"):
                raise Refusal("unexpected_account_privilege")
            if not result.get("email_verified"):
                raise Refusal("email_verification_required")
            account.update(subject=result["id"], creation_state="created")
            write_private("accounts.json", accounts)
        if account["creation_state"] != "created":
            raise Refusal("uncertain_creation_requires_supported_exact_account_lookup")
        code, _, payload, _ = http.request(NYX_ORIGIN, "/api/v1/auth/login", "POST", body={
            "email": account["email"], "password": account["password"], "client": "token",
        })
        if code != 200:
            raise Refusal("account_login_http_" + str(code))
        tokens = decode(payload)
        if tokens.get("user_id") != account["subject"] or not tokens.get("access_token"):
            raise Refusal("account_login_identity_mismatch")
        account["access_token"] = tokens["access_token"]
        account["refresh_token"] = tokens.get("refresh_token")
        write_private("accounts.json", accounts)
        code, _, payload, _ = http.request(NYX_ORIGIN, "/api/v1/users/me", token=account["access_token"])
        me = decode(payload)
        if code != 200 or me.get("id") != account["subject"]:
            raise Refusal("ordinary_account_identity_failed")
        code, _, payload, _ = http.request(
            NYX_ORIGIN, "/api/v1/admin/users/" + account["subject"], token=admin_token)
        verified = decode(payload)
        if (code != 200 or verified.get("id") != account["subject"]
                or verified.get("is_admin") is not False or verified.get("is_operator")
                or verified.get("role", "user") != "user"):
            raise Refusal("ordinary_account_verification_failed")
        code, _, payload, _ = http.request(NYX_ORIGIN, "/api/v1/billing/wallet", token=account["access_token"])
        account["wallet_observation"] = decode(payload) if code == 200 else {"http_status": code}
        write_private("accounts.json", accounts)
        summary.append({"tenant": alias, "ordinary_user": True, "verified": True,
                        "wallet_read_status": code, "funding_added": False})
    if manifest.is_symlink():
        raise Refusal("private_manifest_symlink")
    for parent in (manifest.resolve().parent, *manifest.resolve().parents):
        if (parent / ".git").exists():
            raise Refusal("private_manifest_inside_repository")
    fd = os.open(manifest, os.O_WRONLY | os.O_CREAT | os.O_TRUNC, 0o600)
    with os.fdopen(fd, "w") as stream:
        json.dump({"tenant_a": accounts["a"]["subject"], "tenant_b": accounts["b"]["subject"]}, stream)
        stream.write("\n")
    manifest.chmod(0o600)
    print(json.dumps({"accounts": summary, "subject_manifest_written": True}))


if __name__ == "__main__":
    try:
        parser = argparse.ArgumentParser()
        parser.add_argument("--manifest", required=True, type=Path)
        bootstrap(parser.parse_args().manifest)
    except Refusal as error:
        print("ACCOUNTS BLOCKED: " + str(error))
        raise SystemExit(1)
    except Exception:
        print("ACCOUNTS BLOCKED: local setup failed; inspect private state without printing credentials")
        raise SystemExit(1)
