#!/usr/bin/env python3
"""Dedicated public-desktop/real-receiver fixture, with explicit source/archive reuse."""
import argparse
import ipaddress
import json
import os
from pathlib import Path
import subprocess
import sys
import time
from urllib.parse import urlsplit

PARENT_PREFIX = "store::sandbox_sessions::exclusive::service_tests::endpoint_tests::"
PARENTS = [
    PARENT_PREFIX + "public_disabled_services_and_ownerless_receiver_refuse_new_streams",
    PARENT_PREFIX + "retained_legacy_rows_block_until_close_or_expiry_and_cannot_release_successor",
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
    sys.dont_write_bytecode = True
    from desktop_replica_artifacts import archive_manifest, digest, freeze, identity
    from desktop_replica_results import cleanup_receipts, json_value, require, validate

    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--evidence", type=Path, required=True,
                        help="new private directory outside the checkout; never overwritten")
    parser.add_argument("--mode", choices=["contracts", "one", "two"], default="contracts")
    parser.add_argument("--diagnostics", action="store_true", help="diagnostic collection only")
    parser.add_argument("--cargo-profile", default="test", help="recorded build profile")
    parser.add_argument("--archive", type=Path, help="original archive when CMA_NEXTEST_REUSE=1")
    parser.add_argument("--artifact-manifest", type=Path,
                        help="build receipt: source/toolchain/profile/settings, binary and archive SHA256s")
    args = parser.parse_args()
    root = Path(__file__).resolve().parents[2]
    directory = args.evidence.resolve()
    if directory.is_relative_to(root):
        parser.error("Evidence must remain outside the source checkout")
    os.umask(0o077)
    directory.mkdir(parents=True, exist_ok=False)
    env = os.environ.copy()
    env["CARGO_INCREMENTAL"] = "0"
    env["RUST_MIN_STACK"] = "33554432"
    env["CARGO_TERM_COLOR"] = "never"
    env["CMA_DESKTOP_PUBLIC_MODE"] = args.mode
    env["CMA_DESKTOP_PUBLIC_PHASES"] = "1" if args.diagnostics else "0"
    names = PARENTS if args.mode == "contracts" else [MEASUREMENT]
    parents = "package(=cma) & (" + " | ".join(f"test(={name})" for name in names) + ")"
    selection = parents + f" | (package(=layer4-runtime) & test(={CHILD}))"
    helper = str(root / ".github/scripts/backend-nextest.sh")
    commands = []
    receipt = {"schema": 1, "mode": args.mode, "diagnostics": args.diagnostics,
               "retries": 0, "collection": "not_started", "validation": "failed",
               "qualification": {"correctness": "not_evaluated", "performance": "not_evaluated"},
               "errors": []}
    before = None
    frozen = {}
    hashes = {}
    archive_digest = None
    exit_code = 1

    def run(command, name, output=None):
        started = time.monotonic()
        with (directory / f"{name}.log").open("x") as log:
            log.write(json.dumps({"command": command}) + "\n")
            log.flush()
            if output is None:
                result = subprocess.run(command, cwd=root / "api", env=env, stdout=log, stderr=subprocess.STDOUT)
            else:
                with output.open("x") as stdout:
                    result = subprocess.run(command, cwd=root / "api", env=env, stdout=stdout, stderr=log)
        commands.append({"command": command, "exit_code": result.returncode,
                         "seconds": time.monotonic() - started})
        (directory / "commands.json").write_text(json.dumps(commands, indent=2) + "\n")
        return result.returncode

    try:
        uri = env.get("CHRONO_TEST_MONGODB_URI", "")
        require(ipaddress.ip_address(urlsplit(uri).hostname).is_loopback,
                "Provide an owned disposable Mongo replica set on a loopback address")
        require(not args.diagnostics or args.mode != "contracts", "Diagnostics require a measurement mode")
        before = identity(root, env, args.cargo_profile)
        receipt["identity_before"] = before
        reuse = env.get("CMA_NEXTEST_REUSE")
        require(reuse in (None, "1"), "CMA_NEXTEST_REUSE must be unset or 1")
        require((args.archive is not None and args.artifact_manifest is not None) if reuse else
                args.archive is None and args.artifact_manifest is None,
                "Archive reuse requires both --archive and --artifact-manifest; source builds use neither")
        require(reuse or env.get("CI", "false") != "true", "CI must use the extracted Backend test archive")
        build = ["bash", helper, "list"] if reuse else [
            "cargo", "nextest", "list", "--locked", "-p", "cma", "-p", "layer4-runtime",
            "--cargo-profile", args.cargo_profile,
        ]
        code = run([*build, "--all-features", "--run-ignored", "only",
                    "-E", selection, "--message-format", "json"], "inventory", directory / "inventory.json")
        receipt["inventory_exit_code"] = code
        exit_code = code or 1
        require(code == 0, "Inventory command failed; preserve its first result")
        inventory = json_value((directory / "inventory.json").read_text())
        runtime = selected(inventory, "layer4-runtime", [CHILD])
        cma = selected(inventory, "cma", names)
        if reuse:
            import shutil
            shutil.copy2(root / "api/target/nextest/cargo-metadata.json", directory / "cargo-metadata.json")
        else:
            code = run(["cargo", "metadata", "--locked", "--format-version", "1"], "metadata", directory / "cargo-metadata.json")
            require(code == 0, "Cargo metadata failed")
        metadata = json_value((directory / "cargo-metadata.json").read_text())
        require(Path(metadata["workspace_root"]).resolve() == root / "api", "Wrong artifact workspace")
        frozen, hashes = freeze(directory, inventory, {"runtime": runtime, "cma": cma})
        artifact = {**before, "binaries": hashes, "archive_sha256": None}
        if reuse:
            artifact = archive_manifest(args.artifact_manifest, args.archive, before, hashes)
            archive_digest = artifact["archive_sha256"]
        (directory / "artifact-identity.json").write_text(json.dumps(artifact, indent=2) + "\n")
        require(identity(root, env, args.cargo_profile) == before, "Candidate identity changed during capture")
        env["CMA_DESKTOP_RECEIVER_TEST_BINARY"] = str(frozen["runtime"])
        receivers = directory / "receivers"
        receivers.mkdir()
        env["CMA_DESKTOP_RECEIVER_ROOT"] = str(receivers)
        config = directory / "nextest.toml"
        config.write_text('[profile.default.junit]\npath = ' + json.dumps(str(directory / 'results.xml')) +
                          '\nstore-success-output = true\nstore-failure-output = true\n')
        receipt["nextest_profile"] = {"name": "default", "config_sha256": digest(config)}
        receipt["collection"] = "started"
        code = run(["cargo", "nextest", "run", "--binaries-metadata", str(directory / "binaries-metadata.json"),
                    "--cargo-metadata", str(directory / "cargo-metadata.json"), "--config-file", str(config),
                    "--profile", "default", "--retries", "0", "--run-ignored", "only", "--test-threads", "1",
                    "--no-tests", "fail", "--no-fail-fast", "--success-output", "final", "--failure-output", "final",
                    "-E", parents], "contracts")
        receipt["first_attempt_exit_code"] = code
        receipt["collection"] = "completed" if code == 0 else "failed"
        exit_code = code or 1
        receipt["results"] = validate(directory / "results.xml", names, args.mode, args.diagnostics)
        require(code == 0, "First attempt failed; no retry result can replace it")
        require(not list(receivers.iterdir()), "Receiver roots remain; inspect only this run's owned cleanup")
        receipt["validation"] = "passed"
        receipt["qualification"]["correctness"] = "passed" if args.mode == "contracts" else "not_evaluated"
        if args.mode != "contracts":
            receipt["qualification"]["measurement"] = "diagnostics_only" if args.diagnostics else "valid"
        exit_code = 0
    except (ValueError, OSError, KeyError, TypeError, subprocess.SubprocessError) as error:
        receipt["errors"].append(str(error))
    finally:
        receivers = directory / "receivers"
        if receivers.exists():
            receipt["cleanup"] = {"roots_empty": not list(receivers.iterdir()),
                                  "parents": cleanup_receipts(directory / "results.xml", names)}
            if not receipt["cleanup"]["roots_empty"]:
                receipt["errors"].append("Owned receiver roots remain")
        if archive_digest:
            try:
                receipt["archive"] = {"before": archive_digest, "after": digest(args.archive)}
                require(receipt["archive"]["after"] == archive_digest, "Archive identity drift")
            except (ValueError, OSError) as error:
                receipt["errors"].append(str(error))
        if frozen:
            try:
                after = {name: digest(path) for name, path in frozen.items()}
                receipt["binaries"] = {"before": hashes, "after": after}
                require(after == hashes, "Executed binary identity drift")
            except (ValueError, OSError) as error:
                receipt["errors"].append(str(error))
        if before:
            try:
                receipt["identity_after"] = identity(root, env, args.cargo_profile)
                require(receipt["identity_after"] == before, "Candidate/toolchain/build identity drift")
            except (ValueError, OSError, subprocess.SubprocessError) as error:
                receipt["errors"].append(str(error))
        if receipt["errors"]:
            receipt["validation"] = "failed"
            receipt["qualification"]["correctness"] = "not_evaluated"
            if args.mode != "contracts":
                receipt["qualification"]["measurement"] = "rejected"
            exit_code = receipt.get("first_attempt_exit_code") or exit_code or 1
        receipt["exit_code"] = exit_code
        (directory / "receipt.json").write_text(json.dumps(receipt, indent=2) + "\n")
    print(f"Desktop collection={receipt['collection']}; validation={receipt['validation']}; "
          f"performance=not_evaluated; evidence: {directory}")
    return exit_code


if __name__ == "__main__":
    raise SystemExit(main())
