#!/usr/bin/env python3
"""Qualify the exact CMA scanner contract with a verified upstream release."""
import hashlib
import json
import os
from pathlib import Path
import subprocess
import sys
import tempfile

ROOT = Path(__file__).resolve().parents[2]
NAME = "profile_policy::secret_scanner::tests::pinned_engine_detects_complete_rules_beyond_prefix_markers"
FILTER = "package(=cma) and binary(=cma) and test(=" + NAME + ")"
ARCHIVE = "551f6fc83ea457d62a0d98237cbad105af8d557003051f41f3e7ca7b3f2470eb"
CASES = ["clean values", "generic detection", "package token detection", "annotation cannot bypass detection"]


def require(condition):
    if not condition:
        raise ValueError("qualification input refused")


def unique(pairs):
    require(len({key for key, _ in pairs}) == len(pairs))
    return dict(pairs)


def digest(path):
    result = hashlib.sha256()
    with path.open("rb") as stream:
        for block in iter(lambda: stream.read(1048576), b""):
            result.update(block)
    return result.hexdigest()


def inventory(text):
    data = json.loads(text, object_pairs_hook=unique)
    require(isinstance(data, dict))
    suites = data["rust-suites"]
    require(isinstance(suites, dict))
    target = suites["cma::bin/cma"]
    require(isinstance(target, dict))
    require(all(target.get(k) == v for k, v in {"package-name": "cma", "binary-id": "cma::bin/cma",
        "binary-name": "cma", "kind": "bin", "status": "listed"}.items()))
    selected = []
    for key, suite in suites.items():
        require(isinstance(suite, dict) and isinstance(suite.get("testcases"), dict))
        for name, case in suite["testcases"].items():
            require(isinstance(case, dict) and type(case["ignored"]) is bool
                    and isinstance(case.get("filter-match"), dict))
            match = case["filter-match"]["status"]
            require(match in {"matches", "mismatch"})
            if match == "matches":
                require(case["ignored"])
                selected.append((key, name))
    require(selected == [("cma::bin/cma", NAME)])
    require(isinstance(target["binary-path"], str))
    path = Path(target["binary-path"])
    require(path.is_absolute() and path.is_file())
    return {"path": str(path), "sha256": digest(path)}


def install(directory, env):
    output = directory / "output"
    result = subprocess.run(["bash", str(ROOT / ".github/scripts/install-image-gitleaks.sh")],
        env={**env, "RUNNER_TEMP": str(directory), "GITHUB_PATH": str(directory / "path"),
             "GITHUB_OUTPUT": str(output)}, stdout=subprocess.DEVNULL)
    require(result.returncode == 0)
    lines = output.read_text().splitlines()
    pairs = [line.split("=", 1) for line in lines]
    values = unique(pairs)
    require(set(values) == {"directory", "executable", "receipt"})
    owned = Path(values["directory"]).resolve(strict=True)
    require(owned.parent == directory.resolve() and owned.name.startswith("image-gitleaks."))
    binary = Path(values["executable"]).resolve(strict=True)
    receipt = Path(values["receipt"]).resolve(strict=True)
    require(binary == owned / "gitleaks" and receipt == owned / "identity.json")
    identity = json.loads(receipt.read_text(), object_pairs_hook=unique)
    require(identity["kind"] == "checksum-verified upstream release" and identity["version"] == "8.30.1"
            and identity["archive_sha256"] == ARCHIVE and identity["artifact_platform"] == "linux/amd64"
            and identity["executable"] == str(binary) and identity["binary_sha256"] == digest(binary))
    return binary, identity


def main():
    if len(sys.argv) != 1:
        return 1
    env = {**os.environ, "CARGO_INCREMENTAL": "0"}
    helper = ["bash", str(ROOT / ".github/scripts/backend-nextest.sh")]
    selection = ["--run-ignored", "only", "-E", FILTER]
    phase = "inventory"
    try:
        head = subprocess.check_output(["git", "rev-parse", "HEAD"], cwd=ROOT, text=True).strip()
        listed = subprocess.run(helper + ["list", *selection, "--message-format", "json"],
            cwd=ROOT / "api", env=env, stdout=subprocess.PIPE, text=True)
        if listed.returncode:
            return listed.returncode
        test = inventory(listed.stdout)
        with tempfile.TemporaryDirectory(prefix="cma-scanner-", dir=os.environ.get("RUNNER_TEMP")) as temp:
            phase = "verified installation"
            binary, identity = install(Path(temp), env)
            env["CMA_TEST_GITLEAKS"] = str(binary)
            phase = "real CLI cases"
            cli = subprocess.run(["bash", str(ROOT / ".github/secret-scanner-build-tests.sh"), "--require-real-binary"],
                cwd=ROOT, env=env, stdout=subprocess.PIPE, text=True)
            if cli.returncode:
                return cli.returncode
            actual = [line.removeprefix("SCANNER_CASE_PASS=") for line in cli.stdout.splitlines()
                      if line.startswith("SCANNER_CASE_PASS=")]
            require(actual == CASES)
            require(identity["binary_sha256"] == digest(binary))
            phase = "exact Rust case"
            result = subprocess.run(helper + ["run", *selection, "--test-threads", "1", "--retries", "0",
                "--no-fail-fast", "--success-output", "final"], cwd=ROOT / "api", env=env)
            require(test["sha256"] == digest(Path(test["path"])) and identity["binary_sha256"] == digest(binary))
            require(head == subprocess.check_output(["git", "rev-parse", "HEAD"], cwd=ROOT, text=True).strip())
            receipt = {"source_head": head, "test": test, "engine": identity,
                "cli_cases_passed": actual, "rust_case": NAME, "test_exit": result.returncode}
        require(not Path(temp).exists())
        receipt["owned_installation_removed"] = True
        print(json.dumps(receipt), flush=True)
        return result.returncode
    except (OSError, ValueError, KeyError, TypeError, subprocess.SubprocessError):
        print("Scanner qualification refused during " + phase, file=sys.stderr)
        return 1


if __name__ == "__main__":
    sys.exit(main())
