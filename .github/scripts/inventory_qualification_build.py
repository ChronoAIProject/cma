"""Build identity for dedicated qualifications; ordinary archives are not release proof."""
import hashlib
import json
from pathlib import Path
import re
import subprocess

from inventory_qualification_results import MODES, require, unique


def digest(path):
    result = hashlib.sha256()
    with Path(path).open("rb") as source:
        for block in iter(lambda: source.read(1024 * 1024), b""):
            result.update(block)
    return result.hexdigest()


def source(root):
    require(not subprocess.check_output(["git", "status", "--porcelain"], cwd=root),
            "qualification requires a clean source tree")
    return {key: subprocess.check_output(["git", "rev-parse", ref], cwd=root, text=True).strip()
            for key, ref in [("head", "HEAD"), ("tree", "HEAD^{tree}")]}


def build(root, directory, env):
    require("CMA_NEXTEST_REUSE" not in env, "ordinary archive cannot prove an optimized qualification build")
    identity = {"source": source(root), "profile": "release", "archive": None}
    pins = re.findall(r'^channel\s*=\s*"([^"\n]+)"\s*$', (root / "api/rust-toolchain.toml").read_text(), re.M)
    require(len(pins) == 1 and re.fullmatch(r"\d+\.\d+\.\d+", pins[0]), "invalid toolchain pin")
    pin = pins[0]
    env["RUSTUP_TOOLCHAIN"] = pin
    identity["toolchain"] = subprocess.check_output(["rustc", "+" + pin, "-vV"], env=env, text=True).strip()
    command = ["cargo", "+" + pin, "test", "--locked", "--release", "-p", "cma", "--bin", "cma",
               "--no-run", "--message-format=json"]
    identity["build_command"] = command
    with (directory / "compiler.jsonl").open("w") as output, (directory / "build.log").open("w") as errors:
        result = subprocess.run(command, cwd=root / "api", env=env, stdout=output, stderr=errors)
    require(result.returncode == 0, "optimized test build failed")
    artifacts = [json.loads(line) for line in (directory / "compiler.jsonl").read_text().splitlines() if line.startswith("{")]
    selected = [row for row in artifacts if row.get("reason") == "compiler-artifact" and
                row.get("target", {}).get("name") == "cma" and row.get("target", {}).get("kind") == ["bin"] and
                row.get("profile", {}).get("test") is True and row.get("executable")]
    require(len(selected) == 1, "ambiguous compiled test executable")
    artifact = selected[0]
    require(artifact["profile"]["opt_level"] == "3" and artifact["profile"]["debug_assertions"] is False,
            "compiler did not prove the required optimized profile")
    binary = Path(artifact["executable"]).resolve(strict=True)
    identity.update(binary=str(binary), binary_sha256=digest(binary), compiler_profile=artifact["profile"],
                    compiler_records_sha256=digest(directory / "compiler.jsonl"))
    verify(identity, root)
    return identity


def verify(identity, root):
    require(identity["source"] == source(root), "candidate source changed")
    require(identity["binary_sha256"] == digest(identity["binary"]), "candidate binary changed")


def inventory(text, mode, identity):
    data = json.loads(text, object_pairs_hook=unique)
    suites = data["rust-suites"]
    require(isinstance(suites, dict), "invalid test inventory")
    target = suites["cma::bin/cma"]
    require(all(target.get(k) == v for k, v in {"package-name": "cma", "binary-id": "cma::bin/cma",
        "binary-name": "cma", "kind": "bin", "status": "listed"}.items()), "wrong test binary identity")
    require(Path(target["binary-path"]).resolve() == Path(identity["binary"]), "inventory and build binaries differ")
    selected = []
    for binary, suite in suites.items():
        for name, case in suite["testcases"].items():
            require(type(case["ignored"]) is bool and case["filter-match"]["status"] in {"matches", "mismatch"},
                    "invalid inventory selection")
            if case["filter-match"]["status"] == "matches":
                require(binary == "cma::bin/cma" and case["ignored"], "unexpected selected binary or ordinary test")
                selected.append(name)
    require(set(MODES[mode]) <= set(selected), "required exact qualification case missing")
    if mode in {"availability", "agent-list-profile", "observation-budget"}:
        require(set(selected) == set(MODES[mode]), "unexpected qualification fixture")
    return sorted(selected)
