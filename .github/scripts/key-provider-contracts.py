#!/usr/bin/env python3
"""Run the complete registered KeyIssuer family through the shared archive guard."""
import json
from pathlib import Path
import subprocess
import sys

PREFIX = "auth::keys::fixture_contract_tests::"
FILTER = "package(=cma) and binary(=cma) and test(~" + PREFIX + ")"
ROOT = Path(__file__).resolve().parents[2]


def require(condition):
    if not condition:
        raise ValueError("invalid contract inventory")


def registered():
    entries = json.loads((ROOT / ".github/scripts/mongo-contract-exceptions.json").read_text())
    require(isinstance(entries, dict))
    names = {name for name in entries if name.startswith(PREFIX)}
    require(bool(names))
    for name in names:
        row = entries[name]
        require(isinstance(row, dict) and row.get("kind") == "fixture")
        require(isinstance(row.get("reason"), str) and bool(row["reason"].strip()))
    return names


def validate(inventory, expected):
    require(isinstance(inventory, dict))
    suites = inventory.get("rust-suites")
    require(isinstance(suites, dict) and "cma::bin/cma" in suites)
    target = suites["cma::bin/cma"]
    require(isinstance(target, dict) and all(target.get(key) == value for key, value in {
        "package-name": "cma", "binary-name": "cma", "binary-id": "cma::bin/cma",
        "kind": "bin", "status": "listed",
    }.items()))
    selected = set()
    for suite_id, suite in suites.items():
        require(isinstance(suite, dict) and isinstance(suite.get("testcases"), dict))
        for name, case in suite["testcases"].items():
            require(isinstance(case, dict) and type(case.get("ignored")) is bool)
            match = case.get("filter-match")
            require(isinstance(match, dict) and match.get("status") in {"matches", "mismatch"})
            if match["status"] == "matches":
                require(suite_id == "cma::bin/cma" and name in expected and case["ignored"])
                selected.add(name)
    # Nested cleanup unit tests share the module prefix but are ordinary tests.
    # Every ignored descendant and every direct contract must be registered.
    family = {name for name, case in target["testcases"].items()
              if name.startswith(PREFIX) and (case["ignored"] or "::" not in name[len(PREFIX):])}
    require(family == expected == selected)


def main():
    if len(sys.argv) != 1:
        print("KeyIssuer contract runner refuses arguments", file=sys.stderr)
        return 1
    try:
        expected = registered()
        helper = ["bash", str(ROOT / ".github/scripts/backend-nextest.sh")]
        selection = ["--run-ignored", "only", "-E", FILTER]
        listed = subprocess.run(helper + ["list", *selection, "--message-format", "json"],
                                cwd=ROOT / "api", stdout=subprocess.PIPE, text=True)
        if listed.returncode:
            return listed.returncode
        validate(json.loads(listed.stdout), expected)
    except (OSError, ValueError, KeyError, TypeError):
        print("KeyIssuer contract inventory refused", file=sys.stderr)
        return 1
    print("Validated KeyIssuer contract inventory:", flush=True)
    for name in sorted(expected):
        print(name, flush=True)
    return subprocess.run(helper + ["run", *selection, "--test-threads", "1", "--retries", "0",
                                   "--no-fail-fast", "--success-output", "final"],
                          cwd=ROOT / "api").returncode


if __name__ == "__main__":
    sys.exit(main())
