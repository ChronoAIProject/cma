"""Independent reviewed fingerprints gate every ordinary test actor."""

import hashlib
import json
from pathlib import Path

from safety import CMA_ORIGIN, NYX_ORIGIN, Refusal, decode


def fingerprint(value):
    return hashlib.sha256(value.encode()).hexdigest()


def canonical_hash(value):
    return fingerprint(json.dumps(value, sort_keys=True, separators=(",", ":")))


def approved():
    manifest = json.loads(Path(__file__).with_name("approved.json").read_text())
    if (manifest.get("version") != 1 or manifest.get("origin") != CMA_ORIGIN
            or manifest.get("identity_origin") != NYX_ORIGIN
            or set(manifest.get("actors", {})) != {"a", "b"}):
        raise Refusal("reviewed_approval_manifest_invalid")
    return manifest


def bind_config(accounts, manifest):
    if set(accounts) != {"a", "b"}:
        raise Refusal("two_approved_test_accounts_required")
    for alias in ("a", "b"):
        pin = manifest["actors"][alias]
        account = accounts[alias]
        if (fingerprint(account.get("subject", "")) != pin["subject_sha256"]
                or fingerprint(account.get("email", "")) != pin["email_sha256"]):
            raise Refusal("test_account_does_not_match_reviewed_fingerprint")


def bind_live(p):
    """Only GETs until BOTH identities and exact ordinary authority pass."""
    previous_write_authority = p.http.writes_authorized
    p.http.writes_authorized = False
    p.identities_bound = False
    bind_config(p.accounts, p.approval)
    expected = set(p.approval["capabilities"])
    for alias in ("a", "b"):
        account = p.accounts[alias]
        response = p.http.request(NYX_ORIGIN, "/api/v1/users/me", token=account["access_token"])
        if response[0] != 200:
            raise Refusal("fresh_ordinary_test_session_required")
        me = decode(response[2])
        if not ordinary_profile(me, p.approval["actors"][alias]["subject_sha256"]):
            raise Refusal("live_test_identity_or_privilege_mismatch")
        response = p.http.request(CMA_ORIGIN, "/auth/session", token=account["access_token"])
        if response[0] != 200:
            raise Refusal("ordinary_cma_test_session_required")
        session = decode(response[2])
        if (session.get("authenticated") is not True
                or fingerprint(session.get("subject", "")) != p.approval["actors"][alias]["subject_sha256"]
                or set(session.get("capabilities", [])) != expected):
            raise Refusal("live_cma_identity_or_privilege_mismatch")
    p.identities_bound = True
    p.http.writes_authorized = previous_write_authority


def ordinary_profile(user, subject_hash):
    # NyxID UserProfileResponse resolves platform role and legacy flags live.
    # Missing privilege evidence is not evidence of ordinary authority.
    privileged = {"manage_credit_grants", "manage_catalog", "manage_tools",
                  "read_admin_usage", "read_feature_flags", "create_feature_flags",
                  "manage_all_feature_flags"}
    capabilities = user.get("capabilities", {})
    return (fingerprint(user.get("id", "")) == subject_hash
            and user.get("is_admin") is False and user.get("is_operator") is False
            and user.get("role") == "user" and user.get("is_active") is True
            and isinstance(capabilities, dict)
            and all(capabilities.get(k) is False for k in privileged)
            and not set(capabilities) - privileged - {"billing_available", "enabled_features"})
