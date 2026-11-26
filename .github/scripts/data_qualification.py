#!/usr/bin/env python3
"""Verify the seven SU-DATA case observations against a reviewed inventory.

The reviewed scope contains the frozen candidate, approved policy, and exact
test inventory. Pass its separately supplied SHA-256, an observation index with
relative hash-bound raw JSON references, and a private report destination.
"""
import argparse
from datetime import datetime, timedelta, timezone
import os
from pathlib import Path
import sys

from data_qualification_cases import check_key_recovery, check_quota, validate_inventory
from data_qualification_privacy import check_consent, check_deletion, check_export
from release_candidate_evidence import has_exact_fields, hash_value, timestamp
from release_candidate_packet import EvidenceFiles, decode, digest, outside_checkout, read_file, source_value
from staging_cd_common import matches, require
from staging_cd_receipts import canonical, sha256
from staging_cd_targets import TARGETS

CASES = {
    "TC-063", "TC-064", "TC-065", "TC-119", "TC-135", "TC-136", "TC-137",
}
CUTOFFS = {"TC-063": 16, "TC-064": 19, "TC-065": 20, "TC-119": 20,
           "TC-135": 20, "TC-136": 19, "TC-137": 19}
RECHECK_CUTOFFS = {"TC-064": 26, "TC-065": 26, "TC-119": 24, "TC-136": 26, "TC-137": 24}
SGT = timezone(timedelta(hours=8))


def validate_candidate(value):
    require(has_exact_fields(value, {"source", "images", "configuration_sha256", "flags_sha256", "environment_sha256"}), "Candidate identity is malformed")
    require(source_value(value["source"]) and
            all(hash_value(value[key]) for key in ("configuration_sha256", "flags_sha256", "environment_sha256")),
            "Candidate identity is malformed")
    require(has_exact_fields(value["images"], TARGETS), "Candidate image inventory differs")
    require(all(matches(r"sha256:[0-9a-f]{64}", image) for image in value["images"].values()),
            "Candidate image digest is malformed")


def deadline(day):
    return datetime(2026, 11, day + 1, tzinfo=SGT)


def reviewed_windows(scope):
    phase = scope["phase"]
    require(phase in ("initial", "frozen_recheck"), "Qualification phase differs")
    cutoffs = CUTOFFS if phase == "initial" else RECHECK_CUTOFFS
    require(has_exact_fields(scope["windows"], cutoffs), "Reviewed case windows differ")
    for case, window in scope["windows"].items():
        require(has_exact_fields(window, {"start", "end", "inputs"}), "Reviewed case window is malformed")
        require(isinstance(window["inputs"], dict) and bool(window["inputs"]), "Reviewed inputs are missing")
        start, end = timestamp(window["start"]), timestamp(window["end"])
        require(start < end <= deadline(cutoffs[case]), "Reviewed case window is late or invalid")
        if phase == "frozen_recheck":
            require(start >= deadline(cutoffs[case] - 1) and start >= timestamp(scope["frozen_at"]),
                    "Frozen recheck precedes its scheduled day or freeze")
    return cutoffs


class Observations:
    def __init__(self, scope, index, files):
        require(has_exact_fields(index, {"schema", "scope_sha256", "cases", "initial_report"}), "Observation index is malformed")
        require(index["schema"] == "cma-data-observations-v1", "Observation schema differs")
        require(has_exact_fields(index["cases"], scope["windows"]), "Reviewed case observations are required")
        self.scope, self.index, self.files = scope, index, files
        self.seen_runs, self.seen_hashes = set(), set()
        self.results = {}

    def case(self, case, expected):
        refs = self.index["cases"][case]
        require(has_exact_fields(refs, expected), "Observed case inventory differs from reviewed denominator")
        inputs = self.scope["windows"][case]["inputs"]
        require(has_exact_fields(inputs, expected), "Reviewed inputs differ from case inventory")
        result = {}
        for name, reference in refs.items():
            row = self.files.json(reference)
            require(has_exact_fields(row, {"case", "id", "candidate_sha256", "configuration_sha256", "flags_sha256",
                         "environment_sha256", "run_id", "input_sha256", "seed_sha256", "started_at", "finished_at", "samples",
                         "attempt", "cancelled", "status", "mode", "data"}), "Case receipt is malformed")
            require(row["case"] == case and row["id"] == name
                    and row["candidate_sha256"] == sha256(self.scope["candidate"])
                    and all(row[key] == self.scope["candidate"][key]
                            for key in ("configuration_sha256", "flags_sha256", "environment_sha256")),
                    "Case receipt belongs to another candidate")
            definition = inputs[name]
            require(has_exact_fields(definition, {"input_sha256", "seed_sha256"}), "Reviewed execution input is malformed")
            require(all(hash_value(definition[key]) and row[key] == definition[key] for key in definition),
                    "Execution input or random seed differs")
            require(hash_value(row["run_id"]) and row["run_id"] not in self.seen_runs
                    and reference["sha256"] not in self.seen_hashes, "Case execution or raw receipt was duplicated")
            self.seen_runs.add(row["run_id"])
            self.seen_hashes.add(reference["sha256"])
            started, finished = timestamp(row["started_at"]), timestamp(row["finished_at"])
            window = self.scope["windows"][case]
            cutoffs = CUTOFFS if self.scope["phase"] == "initial" else RECHECK_CUTOFFS
            require(timestamp(window["start"]) <= started < finished <= timestamp(window["end"])
                    and finished < deadline(cutoffs[case]), "Case observation is outside its reviewed window")
            require(type(row["samples"]) is int and row["samples"] > 0
                    and type(row["attempt"]) is int and row["attempt"] == 1 and row["cancelled"] is False
                    and row["status"] == "PASS", "Case receipt is incomplete, cancelled or retry-only")
            require(row["mode"] == self.scope["mode"], "Case execution mode differs from reviewed scope")
            result[name] = row
        self.results[case] = len(result)
        return result


def verify(scope_bytes, expected_scope_hash, index, files):
    require(hash_value(expected_scope_hash) and digest(scope_bytes) == expected_scope_hash,
            "Scope differs from the independently supplied hash")
    scope = decode(scope_bytes)
    require(has_exact_fields(scope, {"schema", "mode", "candidate", "policy", "inventory", "phase", "windows",
                   "frozen_at", "initial_report_sha256"}), "Reviewed SU-DATA scope is malformed")
    require(scope["schema"] == "cma-data-scope-v1" and scope["mode"] in ("synthetic", "isolated-staging"),
            "Reviewed SU-DATA scope is malformed")
    validate_candidate(scope["candidate"])
    cutoffs = reviewed_windows(scope)
    validate_inventory(scope["inventory"], scope["policy"])
    require(index.get("scope_sha256") == expected_scope_hash, "Observation index binds a different scope")
    cases = Observations(scope, index, files)
    inventory, policy = scope["inventory"], scope["policy"]
    initial = None
    if scope["phase"] == "initial":
        require(scope["initial_report_sha256"] is None and scope["frozen_at"] is None
                and index["initial_report"] is None, "Initial qualification has a recheck reference")
        check_quota(cases, inventory, policy)
        check_consent(cases, inventory, policy)
    else:
        reference = index["initial_report"]
        require(hash_value(scope["initial_report_sha256"]) and isinstance(reference, dict)
                and reference.get("sha256") == scope["initial_report_sha256"],
                "Frozen recheck requires an independently pinned initial report")
        initial = files.json(reference)
        require(initial.get("schema") == "cma-data-qualification-report-v1"
                and initial.get("status") == "CONSISTENT" and initial.get("phase") == "initial"
                and initial.get("mode") == scope["mode"] and isinstance(initial.get("cases"), dict)
                and set(initial["cases"]) == CASES and hash_value(initial.get("scope_sha256"))
                and initial.get("candidate_sha256") == sha256(scope["candidate"]),
                "Initial qualification report is incomplete or belongs to another candidate")
        require(initial.get("policy_sha256") == sha256(policy)
                and initial.get("inherited_inventory_sha256") == sha256({key: inventory[key] for key in ("quota", "consents")}),
                "Inherited qualification policy or inventory differs")
        require(isinstance(initial.get("run_ids"), list) and all(hash_value(run) for run in initial["run_ids"])
                and all(type(count) is int and count > 0 for count in initial["cases"].values())
                and len(initial["run_ids"]) == len(set(initial["run_ids"])) == sum(initial["cases"].values()),
                "Initial report run inventory is incomplete")
    check_key_recovery(cases, inventory, policy)
    check_deletion(cases, inventory, policy)
    check_export(cases, inventory, policy)
    require(set(cases.results) == set(cutoffs), "SU-DATA case coverage is incomplete")
    if initial is not None:
        require(not cases.seen_runs & set(initial["run_ids"]), "Frozen recheck reused an initial execution")
    return {"schema": "cma-data-qualification-report-v1", "status": "CONSISTENT",
            "scope": "reviewed_inventory_and_evidence_consistency", "mode": scope["mode"],
            "phase": scope["phase"], "initial_report_sha256": scope["initial_report_sha256"],
            "run_ids": sorted(cases.seen_runs), "policy_sha256": sha256(policy),
            "inherited_inventory_sha256": sha256({key: inventory[key] for key in ("quota", "consents")}),
            "scope_sha256": expected_scope_hash, "candidate_sha256": sha256(scope["candidate"]),
            "source": scope["candidate"]["source"], "cases": cases.results,
            "raw_receipt_count": len(cases.seen_hashes), "observation_sha256": sha256(index)}


def main(argv=None):
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--scope", required=True, type=Path)
    parser.add_argument("--scope-sha256", required=True)
    parser.add_argument("--observation", required=True, type=Path)
    parser.add_argument("--output", required=True, type=Path)
    args = parser.parse_args(argv)
    try:
        output = args.output.parent.resolve(strict=True) / args.output.name
        outside_checkout(output)
        index = decode(read_file(args.observation))
        result = verify(read_file(args.scope), args.scope_sha256, index, EvidenceFiles(args.observation.parent))
        fd = os.open(output, os.O_WRONLY | os.O_CREAT | os.O_EXCL | os.O_NOFOLLOW, 0o600)
        with os.fdopen(fd, "w", encoding="utf-8") as stream:
            stream.write(canonical(result) + "\n")
        print("SU-DATA reviewed inventory and observations are consistent")
        return 0
    except (OSError, ValueError, TypeError, KeyError, AttributeError):
        print("SU-DATA evidence is incomplete or inconsistent; raw diagnostics withheld", file=sys.stderr)
        return 1


if __name__ == "__main__":
    raise SystemExit(main())
