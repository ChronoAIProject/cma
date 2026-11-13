#!/usr/bin/env python3
"""Dedicated public-desktop/real-receiver fixture, with explicit source/archive reuse."""
import argparse
import hashlib
import ipaddress
import json
import os
from pathlib import Path
import subprocess
import time
from urllib.parse import urlsplit

PARENT_PREFIX = "store::sandbox_sessions::exclusive::service_tests::endpoint_tests::"
PARENTS = [
    PARENT_PREFIX + "public_legacy_provider_connects_before_receiver_activation",
    PARENT_PREFIX + "public_replica_handoff_and_revocation_use_actual_receiver",
    PARENT_PREFIX + "public_uncertain_confirm_and_receiver_restart_preserve_fencing",
    PARENT_PREFIX + "delayed_receiver_renewal_ack_cannot_restore_public_authority",
]
CHILD = "link::desktop::tests::child_fixture::desktop_receiver_child"
MEASUREMENT = PARENT_PREFIX + "measurements::matched_public_desktop_connect_windows"


def selected(inventory, package, names):
    matches = []
    found = []
    for suite in inventory["rust-suites"].values():
        if suite["package-name"] != package:
            continue
        for name, case in suite["testcases"].items():
            if case["filter-match"]["status"] == "matches":
                if not case["ignored"]:
                    raise ValueError("Dedicated receiver selection must contain ignored fixtures")
                found.append(name)
        if set(names).issubset(suite["testcases"]):
            matches.append(suite)
    if sorted(found) != sorted(names) or len(matches) != 1:
        raise ValueError("Exact receiver parent/child inventory is missing or changed")
    binary = Path(matches[0]["binary-path"])
    if not binary.is_absolute() or not binary.is_file():
        raise ValueError("Receiver fixture requires the compiled binary from this inventory")
    return binary


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--evidence", type=Path, required=True,
                        help="new dedicated directory; existing evidence is never overwritten")
    parser.add_argument("--mode", choices=["contracts", "legacy", "one", "two"], default="contracts")
    parser.add_argument("--diagnostics", action="store_true", help="separate trace/metric run, never timing acceptance")
    args = parser.parse_args()
    uri = os.environ.get("CHRONO_TEST_MONGODB_URI", "")
    try:
        if not ipaddress.ip_address(urlsplit(uri).hostname).is_loopback:
            raise ValueError()
    except (TypeError, ValueError):
        raise SystemExit("Provide an owned disposable Mongo replica set on a loopback address")
    directory = args.evidence.resolve()
    directory.mkdir(parents=True, exist_ok=False)
    root = Path(__file__).resolve().parents[2]
    env = os.environ.copy()
    env["RUST_MIN_STACK"] = "33554432"
    names = PARENTS if args.mode == "contracts" else [MEASUREMENT]
    if args.diagnostics and args.mode == "contracts":
        raise SystemExit("Diagnostics require an explicit measurement mode")
    env["CMA_DESKTOP_PUBLIC_MODE"] = args.mode
    env["CMA_DESKTOP_PUBLIC_PHASES"] = "1" if args.diagnostics else "0"
    parents = "package(=cma) & (" + " | ".join(f"test(={name})" for name in names) + ")"
    selection = parents + f" | (package(=layer4-runtime) & test(={CHILD}))"
    helper = str(root / ".github/scripts/backend-nextest.sh")
    commands = []

    def run(arguments, name, output=None):
        command = ["bash", helper, *arguments]
        started = time.monotonic()
        with (directory / f"{name}.log").open("x") as log:
            log.write(json.dumps({"command": command}) + "\n")
            log.flush()
            if output is None:
                result = subprocess.run(command, env=env, stdout=log, stderr=subprocess.STDOUT)
            else:
                with output.open("x") as stdout:
                    result = subprocess.run(command, env=env, stdout=stdout, stderr=log)
        commands.append({"command": command, "exit_code": result.returncode,
                         "seconds": time.monotonic() - started})
        (directory / "commands.json").write_text(json.dumps(commands, indent=2) + "\n")
        if result.returncode:
            raise SystemExit(result.returncode)

    run(["list", "--all-features", "--run-ignored", "only", "-E", selection,
         "--message-format", "json"], "inventory", directory / "inventory.json")
    inventory = json.loads((directory / "inventory.json").read_text())
    runtime = selected(inventory, "layer4-runtime", [CHILD])
    cma = selected(inventory, "cma", names)
    env["CMA_DESKTOP_RECEIVER_TEST_BINARY"] = str(runtime)
    receivers = directory / "receivers"
    receivers.mkdir()
    env["CMA_DESKTOP_RECEIVER_ROOT"] = str(receivers)
    def digest(path):
        value = hashlib.sha256()
        with path.open("rb") as source:
            for chunk in iter(lambda: source.read(1024 * 1024), b""):
                value.update(chunk)
        return value.hexdigest()
    (directory / "binaries.json").write_text(json.dumps({
        "runtime": {"path": str(runtime), "sha256": digest(runtime)},
        "cma": {"path": str(cma), "sha256": digest(cma)},
        "mode": args.mode, "diagnostics": args.diagnostics,
        "scope": "Public CMA middleware/router, local IdP/provider proxy, actual child receiver, synthetic display.",
    }, indent=2) + "\n")
    run(["run", "--all-features", "--run-ignored", "only", "--test-threads", "1",
         "--no-fail-fast", "--success-output", "final", "-E", parents], "contracts")
    if list(receivers.iterdir()):
        raise SystemExit("Receiver roots remain; preserve evidence and inspect owned child cleanup")
    print(f"Desktop fixture completed: mode={args.mode}, diagnostics={args.diagnostics}; evidence: {directory}")


if __name__ == "__main__":
    main()
