#!/usr/bin/env python3
"""Verify a release-owner-pinned candidate against independently retained evidence.

Run offline: provide the frozen manifest and its separately communicated SHA-256,
the retained publication records, an observation index, and a private output file.
The observations reference JSON files relative to their index. Successful output
establishes evidence consistency for the supplied release-owner inventory.
"""
import argparse
import hashlib
import json
import os
from pathlib import Path
import stat
import subprocess
import sys

from release_candidate_evidence import (
    check_gate_observations, check_recoveries, check_web_assets, has_exact_fields, hash_value, timestamp,
    validate_checks, verify_matrix,
)
from staging_cd_common import matches, require
from staging_cd_contract import VERSIONS, compatibility
from staging_cd_history import DevelopHistory
from staging_cd_receipts import canonical, sha256, validate_receipt
from staging_cd_set import validate_set
from staging_cd_targets import TARGETS

SCHEMA = "cma-release-candidate-v1"
STATE_FIELDS = {"configuration", "flags", "schema", "external_versions", "reader_floors",
                "runtime_contract"}
MAX_JSON_BYTES = 4 * 1024 * 1024


def digest(value):
    return hashlib.sha256(value).hexdigest()


def source_value(value):
    return matches(r"[0-9a-f]{40}", value)


def read_file(path):
    entry = path.lstat()
    require(stat.S_ISREG(entry.st_mode) and entry.st_size <= MAX_JSON_BYTES,
            "Evidence file is missing or oversized")
    with path.open("rb") as stream:
        data = stream.read(MAX_JSON_BYTES + 1)
    require(0 < len(data) <= MAX_JSON_BYTES, "Evidence file is empty or oversized")
    return data


def decode(data):
    def unique(pairs):
        result = {}
        for key, value in pairs:
            require(key not in result, "Evidence contains duplicate JSON fields")
            result[key] = value
        return result
    def invalid_constant(_):
        raise ValueError("Invalid JSON number")
    return json.loads(data, object_pairs_hook=unique, parse_constant=invalid_constant)


class EvidenceFiles:
    """Resolve only bounded, regular evidence files within the private input directory."""
    def __init__(self, root):
        self.root = root.resolve(strict=True)
        self.hashes = {}

    def read(self, reference):
        require(has_exact_fields(reference, {"path", "sha256"}), "Evidence reference is malformed")
        require(hash_value(reference["sha256"]), "Evidence hash is malformed")
        relative = reference["path"]
        require(isinstance(relative, str), "Evidence path is malformed")
        parts = relative.split("/")
        require(0 < len(parts) <= 8 and all(matches(r"[A-Za-z0-9_-][A-Za-z0-9_.-]{0,127}", part)
                                             for part in parts), "Evidence path is malformed")
        path = self.root
        for part in parts:
            path = path / part
            require(not path.is_symlink(), "Evidence path contains a symbolic link")
        data = read_file(path)
        observed = digest(data)
        require(observed == reference["sha256"], "Evidence file differs from its reference")
        require(self.hashes.setdefault(relative, observed) == observed, "Evidence changed during verification")
        return data

    def json(self, reference):
        return decode(self.read(reference))


def component_identity(receipt):
    validate_receipt(receipt)
    identity = receipt["identity"]
    return {
        "source": receipt["source"], "digest": receipt["digest"],
        "version": identity["version"], "build": identity["build"],
        "publication_sha256": sha256(receipt),
    }


def artifact(receipt):
    return {"source": receipt["source"], "image": receipt["repository"] + "@" + receipt["digest"]}


def local_git(*args, root):
    env = {key: value for key, value in os.environ.items() if key in ("PATH", "LANG", "LC_ALL")}
    env.update(GIT_CONFIG_NOSYSTEM="1", GIT_CONFIG_GLOBAL=os.devnull, GIT_NO_REPLACE_OBJECTS="1",
               GIT_NO_LAZY_FETCH="1", GIT_TERMINAL_PROMPT="0")
    result = subprocess.run(["git", *args], cwd=root, env=env, capture_output=True, text=True,
                            timeout=30, check=False)
    require(result.returncode == 0 and len(result.stdout) <= 16 * 1024 * 1024,
            "Candidate source history is unavailable")
    return result.stdout.strip()


def verify_source_history(head, receipts, root):
    require(source_value(head) and local_git("rev-parse", "--is-shallow-repository", root=root) == "false",
            "Candidate source history is incomplete")
    history = DevelopHistory(head, local_git("rev-list", "--first-parent", head,
                                             root=root).splitlines())
    for receipt in receipts:
        identity = receipt["identity"]
        history.position(identity["source"])
        require(local_git("rev-parse", identity["source"] + "^{tree}", root=root) == identity["tree"],
                "Publication source tree differs")
    return history


def verify_publications(freeze, publications, observed, root):
    require(has_exact_fields(publications, {"schema", "components", "repair"}),
            "Retained publication inventory is malformed")
    require(publications["schema"] == "cma-release-publications-v1", "Publication inventory schema differs")
    receipts = publications["components"]
    validate_set({"schema": "cma-deployment-set-v1", "receipts": receipts})
    require(set(receipts) == TARGETS, "Publication inventory omits a release component")
    require(has_exact_fields(freeze["components"], TARGETS), "Frozen component inventory differs")
    require(has_exact_fields(observed, TARGETS), "Observed component inventory differs")
    for component, receipt in receipts.items():
        expected = component_identity(receipt)
        require(freeze["components"][component] == expected, "Frozen publication identity differs")
        row = observed[component]
        require(has_exact_fields(row, {"identity", "transport", "deployed_digest", "controller_verified"}),
                "Component observation is malformed")
        transport = row["transport"]
        require(has_exact_fields(transport, {"source_digest", "destination_digest", "verified"}),
                "Registry transport result is malformed")
        require(row["identity"] == expected and transport["source_digest"] == expected["digest"]
                and transport["destination_digest"] == expected["digest"] and transport["verified"] is True
                and row["deployed_digest"] == expected["digest"] and row["controller_verified"] is True,
                "Transport or deployed component differs from frozen publication")
    contract = {compatibility(receipts[key]) for key in ("am", "runtime", "runtime-omarchy")}
    require(len(contract) == 1, "Published AM/runtime contracts differ")
    repair = validate_receipt(publications["repair"])
    require(repair["identity"]["component"] == "am" and freeze["repair"] == component_identity(repair),
            "Compatible repair publication identity differs")
    require(compatibility(repair) == next(iter(contract)), "Compatible repair runtime contract differs")
    verify_source_history(freeze["source"], receipts.values(), root)
    # The independently approved repair may be a successor of the serving
    # candidate. Its pinned publication lineage must include both sources.
    repair_history = verify_source_history(freeze["repair_lineage_head"], [repair], root)
    repair_history.position(freeze["source"])
    return artifact(receipts["am"]), artifact(repair), dict(zip(VERSIONS, next(iter(contract))))


def verify_packet(freeze_bytes, freeze_sha, publications, observation, files, root):
    require(hash_value(freeze_sha) and digest(freeze_bytes) == freeze_sha,
            "Freeze differs from the independently supplied hash")
    freeze = decode(freeze_bytes)
    require(has_exact_fields(freeze, {
        "schema", "source", "environment", "frozen_at", "components", "state", "repair",
        "repair_lineage_head", "web_assets", "matrix", "checks", "previous_candidate_sha256",
    }), "Frozen candidate inventory is malformed")
    require(freeze["schema"] == SCHEMA and source_value(freeze["source"])
            and hash_value(freeze["environment"]), "Frozen candidate identity is malformed")
    frozen_at = timestamp(freeze["frozen_at"])
    candidate_sha = sha256({key: freeze[key] for key in ("source", "environment", "components", "state",
                                                         "repair", "repair_lineage_head", "web_assets", "matrix")})
    previous = freeze["previous_candidate_sha256"]
    require(previous is None or (hash_value(previous) and previous != candidate_sha),
            "Previous freeze identity is malformed")
    require(has_exact_fields(observation, {
        "schema", "candidate_sha256", "environment", "observed_at", "components",
        "state", "web_assets", "matrix", "checks", "recoveries",
    }), "Candidate observation index is malformed")
    observed_at = timestamp(observation["observed_at"])
    require(observation["schema"] == "cma-release-candidate-observation-v1"
            and observation["candidate_sha256"] == candidate_sha
            and observation["environment"] == freeze["environment"]
            and observed_at >= frozen_at, "Observation belongs to another candidate")
    candidate, repair, contract = verify_publications(freeze, publications, files.json(observation["components"]), root)
    require(has_exact_fields(freeze["state"], STATE_FIELDS), "Frozen configuration inventory differs")
    require(has_exact_fields(observation["state"], STATE_FIELDS), "Observed configuration inventory differs")
    for key, expected_hash in freeze["state"].items():
        require(hash_value(expected_hash), "Frozen state hash is malformed")
        reference = observation["state"][key]
        files.read(reference)
        require(reference["sha256"] == expected_hash, "Configuration, schema or reader floor differs")
    require(files.json(observation["state"]["runtime_contract"]) == contract,
            "Observed runtime contract differs from publication")
    web = check_web_assets(freeze, observation["web_assets"], files)
    validate_checks(freeze["checks"], previous)
    checks = check_gate_observations(freeze, candidate_sha, observation["checks"], files, observed_at)
    matrix = verify_matrix(freeze, observation["matrix"], files, candidate, repair, observed_at)
    recoveries = check_recoveries(freeze, candidate_sha, observation["recoveries"], files, repair, observed_at)
    return {
        "schema": "cma-release-candidate-report-v1",
        "status": "CONSISTENT",
        "scope": "reviewed_inventory_and_evidence_consistency",
        "freeze_sha256": freeze_sha, "candidate_sha256": candidate_sha, "source": freeze["source"],
        "components": sorted(TARGETS), "required_checks": checks,
        "retained_assets": web, "reader_matrix": matrix, "recovery_rehearsals": recoveries,
        "publications_sha256": sha256(publications), "observation_sha256": sha256(observation),
        "evidence_sha256": sorted(set(files.hashes.values())),
    }


def outside_checkout(path):
    require(not any((parent / ".git").exists() for parent in (path.parent, *path.parent.parents)),
            "Release evidence output must be outside the repository")


def main(argv=None):
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--freeze", required=True, type=Path)
    parser.add_argument("--freeze-sha256", required=True)
    parser.add_argument("--publications", required=True, type=Path)
    parser.add_argument("--observation", required=True, type=Path)
    parser.add_argument("--output", required=True, type=Path)
    parser.add_argument("--repository", type=Path, default=Path.cwd())
    args = parser.parse_args(argv)
    try:
        output = args.output.parent.resolve(strict=True) / args.output.name
        outside_checkout(output)
        files = EvidenceFiles(args.observation.parent)
        report = verify_packet(read_file(args.freeze), args.freeze_sha256,
                               decode(read_file(args.publications)), decode(read_file(args.observation)),
                               files, args.repository)
        fd = os.open(output, os.O_WRONLY | os.O_CREAT | os.O_EXCL | os.O_NOFOLLOW, 0o600)
        with os.fdopen(fd, "w", encoding="utf-8") as stream:
            stream.write(canonical(report) + "\n")
        print("Candidate inventory and evidence are consistent")
        return 0
    except (OSError, ValueError, TypeError, KeyError, AttributeError, subprocess.SubprocessError):
        print("Candidate evidence is incomplete or inconsistent; raw diagnostics withheld", file=sys.stderr)
        return 1


if __name__ == "__main__":
    raise SystemExit(main())
