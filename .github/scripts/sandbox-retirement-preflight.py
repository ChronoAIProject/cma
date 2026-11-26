#!/usr/bin/env python3
"""Inspect an ordered private Sandbox retirement inventory via the admin API.

Supply the bearer credential in CMA_RETIREMENT_READ_TOKEN. The inventory has
format 1, a UTC idle_before timestamp and an ordered members array. Each member
specifies id, owner, created_at, revision, status, last_activity_at, image,
runtime_sha256 and home_sha256. The digests use sorted compact JSON of the
corresponding objects in the Sandbox admin detail. The report contains only
hashes and outcomes. Preflight observations grant no execution authority.
"""

import argparse
import hashlib
import json
import os
import re
import signal
import stat
from datetime import datetime, timezone
from pathlib import Path
from urllib.error import HTTPError, URLError
from urllib.parse import urlsplit
from urllib.request import HTTPRedirectHandler, ProxyHandler, Request, build_opener


MAX_INPUT = 256 * 1024
MAX_RESPONSE = 2 * 1024 * 1024
MEMBER_FIELDS = {
    "id", "owner", "created_at", "revision", "status", "last_activity_at", "image",
    "runtime_sha256", "home_sha256",
}
ID = re.compile(r"sbx_[a-zA-Z0-9_-]{1,120}\Z")
DIGEST = re.compile(r"[0-9a-f]{64}\Z")
IMAGE = re.compile(r"[^\s]+@sha256:[0-9a-f]{64}\Z")
UTC_TIMESTAMP = re.compile(r"\d{4}-\d\d-\d\dT\d\d:\d\d:\d\d(?:\.\d{1,6})?Z\Z")
QUIET_AGENTS = {"ready", "paused", "stopped", "failed", "deleted"}


def digest(data):
    return hashlib.sha256(data).hexdigest()


def object_digest(value):
    return digest(json.dumps(value, sort_keys=True, separators=(",", ":")).encode())


def unique_fields(pairs):
    value = dict(pairs)
    if len(value) != len(pairs):
        raise ValueError("duplicate JSON field")
    return value


def utc_timestamp(value):
    if not isinstance(value, str) or UTC_TIMESTAMP.fullmatch(value) is None:
        raise ValueError("expected a UTC timestamp")
    return datetime.fromisoformat(value[:-1] + "+00:00")


def read_inventory(path):
    fd = os.open(path, os.O_RDONLY | os.O_NOFOLLOW | os.O_NONBLOCK)
    with os.fdopen(fd, "rb") as stream:
        metadata = os.fstat(stream.fileno())
        if not stat.S_ISREG(metadata.st_mode) or metadata.st_uid != os.geteuid() or metadata.st_mode & 0o077:
            raise ValueError("inventory must be a private regular file")
        data = stream.read(MAX_INPUT + 1)
    if len(data) > MAX_INPUT:
        raise ValueError("inventory exceeds its size bound")
    return data


def parse_inventory(data, now):
    value = json.loads(data, object_pairs_hook=unique_fields)
    if (
        not isinstance(value, dict)
        or set(value) != {"format", "idle_before", "members"}
        or type(value["format"]) is not int
        or value["format"] != 1
    ):
        raise ValueError("invalid inventory format")
    cutoff = utc_timestamp(value["idle_before"])
    members = value["members"]
    if cutoff >= now or not isinstance(members, list) or not 1 <= len(members) <= 128:
        raise ValueError("invalid inactivity cutoff or number of members")
    seen = set()
    for member in members:
        if not isinstance(member, dict) or set(member) != MEMBER_FIELDS:
            raise ValueError("invalid inventory member")
        if (
            not all(isinstance(member[key], str) for key in MEMBER_FIELDS - {"revision"})
            or not ID.fullmatch(member["id"])
            or member["id"] in seen
            or not member["owner"].strip()
            or type(member["revision"]) is not int
            or member["revision"] <= 0
            or member["status"] not in {"active", "failed"}
            or not IMAGE.fullmatch(member["image"])
            or not DIGEST.fullmatch(member["runtime_sha256"])
            or not DIGEST.fullmatch(member["home_sha256"])
        ):
            raise ValueError("invalid or repeated inventory member")
        utc_timestamp(member["created_at"])
        utc_timestamp(member["last_activity_at"])
        seen.add(member["id"])
    return cutoff, members


class NoRedirect(HTTPRedirectHandler):
    def redirect_request(self, req, fp, code, msg, headers, newurl):
        return None


def admin_origin(raw):
    url = urlsplit(raw)
    loopback_http = url.scheme == "http" and url.hostname == "127.0.0.1"
    if (
        not (url.scheme == "https" or loopback_http)
        or not url.hostname
        or url.username is not None
        or url.password is not None
        or raw != f"{url.scheme}://{url.netloc}"
        or any(ch.isspace() for ch in raw)
    ):
        raise ValueError("expected an absolute HTTPS origin or loopback HTTP origin")
    return raw


def inspect_member(member, detail, cutoff):
    sandbox = detail["sandbox"]
    if any(sandbox[key] != member[key] for key in ("id", "owner", "created_at")):
        return "identity_changed"
    if sandbox["status"] in {"retiring", "retired"}:
        return "observe_existing_retirement"
    activity = utc_timestamp(sandbox["last_activity_at"])
    agents = detail["agents"]
    if not isinstance(agents, list):
        raise ValueError("invalid Agent inventory")
    if activity >= cutoff or any(
        agent["status"] not in QUIET_AGENTS or utc_timestamp(agent["updated_at"]) >= cutoff
        for agent in agents
    ):
        return "recent_or_active_work"
    if any(sandbox[key] != member[key] for key in ("status", "last_activity_at", "revision")):
        return "snapshot_changed"
    runtime = sandbox.get("runtime")
    home = sandbox["lifecycle"].get("home")
    if not isinstance(runtime, dict) or not isinstance(home, dict):
        return "runtime_or_home_unavailable"
    pool = home.get("native_pool")
    claim = pool.get("claim") if isinstance(pool, dict) else None
    receipt = home.get("receipt")
    runtime_id = runtime.get("runtime_id")
    if (
        not isinstance(claim, dict)
        or not claim.get("uid")
        or not isinstance(receipt, dict)
        or not receipt.get("storage_reference")
        or home.get("released") is not False
        or pool.get("cleaned") is not False
        or pool.get("cleanup") is not None
        or not isinstance(runtime_id, str)
        or not runtime_id.startswith("agent-sandbox:")
    ):
        return "runtime_or_home_unavailable"
    if (
        runtime.get("runtime_version") != member["image"]
        or pool["request"]["pool"]["spec"]["image"]["uri"] != member["image"]
        or object_digest(runtime) != member["runtime_sha256"]
        or object_digest(home) != member["home_sha256"]
    ):
        return "runtime_or_home_changed"
    return "eligible_for_review"


def preflight(origin, token, data, now):
    cutoff, members = parse_inventory(data, now)
    client = build_opener(ProxyHandler({}), NoRedirect())
    rows = []
    for member in members:
        request = Request(
            f"{origin}/api/v1/admin/sandboxes/{member['id']}",
            headers={"Authorization": f"Bearer {token}", "Accept": "application/json"},
        )
        try:
            with client.open(request, timeout=5) as response:
                if response.status != 200:
                    raise ValueError("unexpected response status")
                body = response.read(MAX_RESPONSE + 1)
            if len(body) > MAX_RESPONSE:
                raise ValueError("response exceeds its size bound")
            outcome = inspect_member(member, json.loads(body, object_pairs_hook=unique_fields), cutoff)
        except HTTPError as error:
            outcome = "not_found_or_hidden" if error.code == 404 else "observation_unavailable"
            error.close()
        except (URLError, OSError, KeyError, TypeError, ValueError):
            outcome = "observation_unavailable"
        rows.append({"member_sha256": object_digest(member), "outcome": outcome})
    return {"format": 1, "inventory_sha256": digest(data), "rows": rows, "execution_authorized": False}


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("origin", type=admin_origin)
    parser.add_argument("inventory", type=Path)
    parser.add_argument("output", type=Path)
    args = parser.parse_args()
    token = os.environ.get("CMA_RETIREMENT_READ_TOKEN", "")
    if not token or any(ch.isspace() for ch in token):
        parser.exit(2, "A read credential is required.\n")
    try:
        data = read_inventory(args.inventory)
        signal.signal(signal.SIGALRM, lambda *_: parser.exit(2, "Inventory observation timed out.\n"))
        signal.alarm(120)
        try:
            report = preflight(args.origin, token, data, datetime.now(timezone.utc))
        finally:
            signal.alarm(0)
        fd = os.open(args.output, os.O_WRONLY | os.O_CREAT | os.O_EXCL, 0o600)
        with os.fdopen(fd, "w", encoding="utf-8") as output:
            json.dump(report, output, indent=2)
            output.write("\n")
            output.flush()
            os.fsync(output.fileno())
    except (OSError, ValueError):
        parser.exit(2, "The private inventory or report could not be used.\n")
    return int(any(row["outcome"] == "observation_unavailable" for row in report["rows"]))


if __name__ == "__main__":
    raise SystemExit(main())
