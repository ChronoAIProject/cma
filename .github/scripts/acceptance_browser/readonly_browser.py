"""Read-only browser entrypoint. No login, provisioning or acceptance writes."""

import argparse
import datetime
import fcntl
import hashlib
import json
import os
from pathlib import Path
import selectors
import subprocess
import sys
import time

HERE = Path(__file__).resolve().parent
sys.path.insert(0, str(HERE.parent / "acceptance"))
from evidence import Evidence
from safety import Refusal, outside_quiet_window, private_root
from probe import Probe
from artifacts import AuditedHttp, attest
from binding import bind_browser, existing_fixture
from validation import PHASES, assess, validate

REASONS = {
    "asset_reference_not_allowlisted",
    "build_info_unavailable",
    "build_info_invalid",
    "root_unavailable",
    "entry_assets_missing",
    "asset_count_limit",
    "asset_dependency_parser_refused",
    "asset_status_or_size_limit",
    "artifact_changed_during_observation",
    "request_transport_failure",
    "response_body_limit",
    "quiet_window_0200_0259_sgt",
    "request_budget_exhausted",
    "browser_observation_incomplete",
    "browser_output_schema_refused",
    "browser_protocol_refused",
    "browser_output_limit",
    "fresh_ordinary_test_session_required",
    "fresh_dedicated_cookie_required",
    "existing_dedicated_agent_required",
    "existing_fixture_not_readable",
    "existing_settled_owned_fixture_required",
}


def safe_reason(error):
    return (
        str(error)
        if isinstance(error, Refusal) and str(error) in REASONS
        else "unrecognized_refusal"
    )


def now():
    return datetime.datetime.now(datetime.timezone.utc).isoformat()


def source_hash():
    return hashlib.sha256(
        b"".join(
            path.name.encode() + b"\0" + path.read_bytes()
            for path in sorted(HERE.iterdir())
            if path.suffix in (".py", ".mjs")
        )
    ).hexdigest()


def browser_run(mode, probe=None):
    config = {"mode": mode}
    if mode == "agent":
        bind_browser(probe)
        config.update(
            agent=existing_fixture(probe), cookie=probe.accounts["a"]["cma_cookie"]
        )
    # Credentials are transported only through an anonymous stdin pipe. Browser
    # debug, tracing, proxy and persistent-profile environment are not inherited.
    environment = {
        key: os.environ[key]
        for key in ("PATH", "HOME", "TMPDIR", "DISPLAY")
        if key in os.environ
    }
    command = [
        "npm",
        "exec",
        "--offline",
        "--package=@playwright/test@1.58.2",
        "--package=puppeteer-core@25.17.0",
        "--",
        "node",
        str(HERE / "browser.mjs"),
    ]
    child = subprocess.Popen(
        command,
        stdin=subprocess.PIPE,
        stdout=subprocess.PIPE,
        stderr=subprocess.DEVNULL,
        env=environment,
        start_new_session=True,
    )
    result = None
    buffer = b""
    deadline = time.monotonic() + 600
    selector = selectors.DefaultSelector()
    selector.register(child.stdout, selectors.EVENT_READ)

    def send(value):
        child.stdin.write(json.dumps(value).encode() + b"\n")
        child.stdin.flush()

    try:
        send(config)
        while time.monotonic() < deadline:
            if not selector.select(timeout=1):
                if child.poll() is not None:
                    break
                continue
            data = os.read(child.stdout.fileno(), 65536)
            if not data:
                break
            buffer += data
            if len(buffer) > 2 * 1024 * 1024:
                raise Refusal("browser_output_limit")
            while b"\n" in buffer:
                line, buffer = buffer.split(b"\n", 1)
                message = json.loads(line)
                if set(message) == {"event", "phase"} and message["event"] == "bind":
                    if probe is None or message["phase"] not in PHASES:
                        raise Refusal("browser_protocol_refused")
                    try:
                        outside_quiet_window()
                        bind_browser(probe)
                        existing_fixture(probe)
                    except Exception:
                        send({"ok": False})
                    else:
                        send({"ok": True})
                elif (
                    set(message) == {"event", "result"}
                    and message["event"] == "result"
                    and result is None
                ):
                    result = validate(message["result"])
                else:
                    raise Refusal("browser_protocol_refused")
        if result is None:
            raise Refusal("browser_observation_incomplete")
        return result
    finally:
        selector.close()
        child.stdin.close()
        try:
            child.wait(timeout=10)
        except subprocess.TimeoutExpired:
            # This process group belongs exclusively to this ephemeral browser.
            import signal

            os.killpg(child.pid, signal.SIGTERM)
            child.wait(timeout=10)
        child.stdout.close()


def run(mode):
    outside_quiet_window()
    artifact = {
        "version": 1,
        "purpose": "current_web_readonly_dry_run",
        "started_utc": now(),
        "mode": mode,
        "matched_baseline": False,
        "server_join": False,
        "release_acceptance": False,
        "source_sha256": source_hash(),
    }
    root = private_root()
    fd = os.open(root / "runner.lock", os.O_CREAT | os.O_RDWR | os.O_NOFOLLOW, 0o600)
    with os.fdopen(fd, "w") as lock:
        fcntl.flock(lock, fcntl.LOCK_EX | fcntl.LOCK_NB)
        if mode == "public":
            http = AuditedHttp()
            try:
                artifact["artifact"] = attest(http)
            except Exception as error:
                artifact["artifact_refusal"] = safe_reason(error)
            artifact["artifact_reads"] = http.reads
            browser = browser_run(mode)
        else:
            p = Probe()
            browser = browser_run(mode, p)
            artifact["actor_sha256"] = p.approval["actors"]["a"]["subject_sha256"]
            artifact["identity_fixture_gets"] = p.http.count
        artifact["browser"] = browser
        artifact["cases"] = assess(browser, mode == "agent")
    artifact["finished_utc"] = now()
    if source_hash() != artifact["source_sha256"]:
        raise Refusal("source_changed_during_observation")
    return artifact


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("mode", choices=("public", "agent"))
    parser.add_argument("--output", required=True)
    args = parser.parse_args()
    # Reserve the output before ANY HTTP request or account file read.
    with Evidence(args.output) as evidence:
        try:
            artifact = run(args.mode)
        except Exception:
            evidence.publish(
                {
                    "status": "REFUSED",
                    "reason": "readonly_preflight_or_observation_incomplete",
                    "release_acceptance": False,
                }
            )
            print("READONLY REFUSED: see private evidence; no write fallback")
            return 1
        evidence.publish(artifact)
    print("READONLY RECORDED: aggregate evidence only; not release acceptance")
    return 0


if __name__ == "__main__":
    try:
        sys.exit(main())
    except Exception:
        print("READONLY REFUSED: output preflight failed")
        sys.exit(1)
