#!/usr/bin/env python3
"""Check regional recovery evidence against an independently reviewed inventory."""
import argparse
from datetime import datetime, timedelta, timezone
import os
from pathlib import Path
import sys

from release_candidate_evidence import has_exact_fields, hash_value, timestamp
from release_candidate_packet import EvidenceFiles, decode, digest, outside_checkout, read_file, source_value
from staging_cd_common import matches, require
from staging_cd_receipts import canonical, sha256
from staging_cd_targets import TARGETS

SCOPES = {"control_plane", "runtime_region", "database_service"}
DEPENDENCIES = {"identity", "model_gateway", "database", "public_edge"}
HOME_CLASSES = {"warm", "cold", "resumed"}
STAGES = ("quota", "network", "identity", "certificates", "dns", "secrets", "monitoring", "consumer_service")
CUSTODY = {"mongo", "objects", "keys", "homes"}
SGT = timezone(timedelta(hours=8))


def hashes(values):
    return all(hash_value(value) for value in values)


def nonnegative(value):
    return type(value) is int and value >= 0


def validate_scope(scope):
    require(has_exact_fields(scope, {"schema", "mode", "phase", "candidate", "window", "initial_report_sha256", "frozen_at", "inventory",
                   "home_sample", "coverage_sha256", "backup_campaign_sha256", "copy_changes", "failure_scopes", "commitments", "exercise_copy_receipts"}), "Reviewed recovery scope is incomplete")
    require(scope["schema"] == "cma-regional-recovery-v1" and scope["mode"] in ("synthetic", "isolated-staging")
            and scope["phase"] in ("initial", "frozen_recheck"), "Recovery scope identity differs")
    candidate = scope["candidate"]
    require(has_exact_fields(candidate, {"source", "images", "configuration_sha256", "flags_sha256", "environment_sha256"}), "Candidate is incomplete")
    require(has_exact_fields(candidate["images"], TARGETS), "Candidate image inventory differs")
    require(source_value(candidate["source"]) and hashes(candidate[key] for key in
            ("configuration_sha256", "flags_sha256", "environment_sha256"))
            and all(matches(r"sha256:[0-9a-f]{64}", value) for value in candidate["images"].values()), "Candidate identity is malformed")
    window = scope["window"]
    require(has_exact_fields(window, {"start", "end"}), "Qualification window is incomplete")
    start, end = timestamp(window["start"]), timestamp(window["end"])
    require(start < end, "Qualification window is invalid")
    if scope["phase"] == "initial":
        require(end <= datetime(2026, 11, 24, tzinfo=SGT) and scope["initial_report_sha256"] is None
                and scope["frozen_at"] is None, "Initial qualification is late or mislabelled")
    else:
        require(datetime(2026, 11, 26, tzinfo=SGT) <= start < end <= datetime(2026, 11, 27, tzinfo=SGT)
                and start >= timestamp(scope["frozen_at"]) and hash_value(scope["initial_report_sha256"]),
                "Frozen recheck is outside its scheduled window")
    inventory = scope["inventory"]
    require(isinstance(inventory, dict) and len(inventory) >= 10 and hashes(inventory), "Retained home inventory is incomplete")
    volumes, backups = set(), set()
    for home in inventory.values():
        require(has_exact_fields(home, {"class", "used_bytes", "owner_sha256", "home_sha256", "native_sha256", "manifest_sha256", "receipts_sha256",
                      "source_storage_sha256", "source_domain_sha256", "backup_sha256", "backup_storage_sha256", "backup_domain_sha256"}), "Reviewed home is malformed")
        require(home["class"] in HOME_CLASSES and nonnegative(home["used_bytes"])
                and hashes(home[key] for key in set(home) - {"class", "used_bytes"}), "Reviewed home identity is malformed")
        require(home["source_storage_sha256"] not in volumes and home["backup_sha256"] not in backups
                and home["source_domain_sha256"] != home["backup_domain_sha256"],
                "Reviewed storage identity is reused")
        volumes.add(home["source_storage_sha256"])
        backups.add(home["backup_sha256"])
    copies = {home["backup_storage_sha256"] for home in inventory.values()}
    require(len(copies) == len(inventory) and not volumes & (backups | copies) and not backups & copies and sum(home["used_bytes"] for home in inventory.values()) > 0,
            "Independent backup identity or equivalent volume size is missing")
    require(has_exact_fields(scope["copy_changes"], inventory), "Reviewed changed-byte inventory differs")
    for change in scope["copy_changes"].values():
        require(has_exact_fields(change, {"steady", "peak"}), "Reviewed change volume is incomplete")
        require(nonnegative(change["steady"]) and nonnegative(change["peak"]) and change["peak"] >= change["steady"],
                "Reviewed change volume is malformed")
    require(sum(change["steady"] for change in scope["copy_changes"].values()) > 0
            and sum(change["peak"] for change in scope["copy_changes"].values()) > 0, "Incremental workload is unproven")
    sample = scope["home_sample"]
    require(isinstance(sample, list) and len(sample) == len(set(sample)) == 10 and set(sample) <= set(inventory)
            and {inventory[home]["class"] for home in sample} == HOME_CLASSES, "Reviewed local sample differs")
    require(has_exact_fields(scope["failure_scopes"], SCOPES), "Three regional fault scopes are required")
    for row in scope["failure_scopes"].values():
        require(has_exact_fields(row, {"source_domain_sha256", "recovery_domain_sha256", "review_sha256", "quota_bytes", "consumer_order"}), "Reviewed failure scope is incomplete")
        require(hashes(row[key] for key in ("source_domain_sha256", "recovery_domain_sha256", "review_sha256"))
                and row["source_domain_sha256"] != row["recovery_domain_sha256"]
                and nonnegative(row["quota_bytes"]) and row["quota_bytes"] >= sum(home["used_bytes"] for home in inventory.values())
                and isinstance(row["consumer_order"], list) and bool(row["consumer_order"]) and hashes(row["consumer_order"])
                and len(row["consumer_order"]) == len(set(row["consumer_order"])), "Reviewed recovery capacity or scope differs")
    require(has_exact_fields(scope["exercise_copy_receipts"], SCOPES | set(sample)), "Reviewed exercise backup observation inventory differs")
    require(hashes(scope["exercise_copy_receipts"].values()), "Exercise copies are unpinned")
    require(has_exact_fields(scope["commitments"], DEPENDENCIES), "Critical dependency inventory differs")
    require(hashes(scope["commitments"].values()) and hashes(scope[key] for key in ("coverage_sha256", "backup_campaign_sha256")), "Reviewed external evidence is unpinned")


def verify_coverage(scope, reference, files):
    require(reference["sha256"] == scope["coverage_sha256"], "Backup coverage evidence differs from review")
    coverage = files.json(reference)
    count = len(scope["inventory"])
    require(coverage.get("mode") == ("fixture" if scope["mode"] == "synthetic" else "cloud")
            and coverage.get("status") == "PASS" and coverage.get("retained_homes") == coverage.get("backed_homes") == count
            and coverage.get("coverage") == 1 and coverage.get("orphan_count") == coverage.get("ownership_unknown_count") == 0,
            "Backup coverage does not reconcile all retained homes")
    if scope["mode"] == "isolated-staging":
        require(coverage.get("catalog_export_status") == "AVAILABLE" and hash_value(coverage.get("catalog_artifact_sha256")),
                "Cloud coverage lacks its exported catalog identity")
    homes = coverage.get("homes")
    require(isinstance(homes, list) and len(homes) == count, "Backup coverage row inventory differs")
    observed = {}
    for row in homes:
        require(isinstance(row, dict) and row.get("reasons") == [] and row.get("sandbox_sha256") not in observed,
                "Backup coverage row is duplicated or unresolved")
        observed[row["sandbox_sha256"]] = row
    require(has_exact_fields(observed, scope["inventory"]), "Backup coverage omits a reviewed home")
    for key, home in scope["inventory"].items():
        require(observed[key].get("owner_sha256") == home["owner_sha256"]
                and observed[key].get("backup_sha256") == home["backup_sha256"], "Backup coverage row identity differs")


def verify_backup_campaign(scope, reference, files):
    require(reference["sha256"] == scope["backup_campaign_sha256"], "Backup campaign differs from review")
    row = files.json(reference)
    require(has_exact_fields(row, {"mode", "candidate_sha256", "started_at", "finished_at", "homes", "api_load_sha256", "quota_sha256", "cost_sha256"}), "Backup campaign is incomplete")
    start, finish = timestamp(row["started_at"]), timestamp(row["finished_at"])
    require(row["mode"] == scope["mode"] and row["candidate_sha256"] == sha256(scope["candidate"])
            and timestamp(scope["window"]["start"]) <= start < finish <= timestamp(scope["window"]["end"])
            and (scope["phase"] == "frozen_recheck" or finish < datetime(2026, 11, 20, tzinfo=SGT))
            and hashes(row[key] for key in ("api_load_sha256", "quota_sha256", "cost_sha256")),
            "Backup qualification window or resource evidence differs")
    require(has_exact_fields(row["homes"], scope["inventory"]), "Backup campaign omits a retained home")
    receipts, copies, backup_ids = set(), set(), set()
    known_copies = {}
    sources = {home["source_storage_sha256"] for home in scope["inventory"].values()}
    totals = {kind: 0 for kind in ("full", "steady", "peak")}
    for key, captures in row["homes"].items():
        expected = scope["inventory"][key]
        sizes = {"full": expected["used_bytes"], **scope["copy_changes"][key]}
        require(isinstance(captures, list) and len(captures) == len(sizes), "Backup capture history is incomplete")
        previous = previous_capture = None
        for kind, capture in zip(("full", "steady", "peak"), captures):
            require(has_exact_fields(capture, {"kind", "started_at", "finished_at", "bytes", "receipt_sha256", "backup_sha256", "storage_sha256",
                             "domain_sha256", "readable", "manifest_sha256", "checkpoint_sha256", "captured_at"}), "Backup capture evidence is incomplete")
            began, ended = timestamp(capture["started_at"]), timestamp(capture["finished_at"])
            captured = timestamp(capture["captured_at"])
            require(capture["kind"] == kind and start <= began < ended <= finish and ended - began <= timedelta(minutes=30)
                    and (previous is None or timedelta(0) < began - previous <= timedelta(minutes=30))
                    and began <= captured <= ended
                    and (previous_capture is None or timedelta(0) < captured - previous_capture <= timedelta(minutes=30))
                    and hash_value(capture["checkpoint_sha256"])
                    and type(capture["bytes"]) is int and capture["bytes"] == sizes[kind] and capture["readable"] is True
                    and hashes(capture[field] for field in ("receipt_sha256", "backup_sha256", "storage_sha256", "domain_sha256", "manifest_sha256"))
                    and capture["domain_sha256"] != expected["source_domain_sha256"]
                    and capture["storage_sha256"] not in sources and capture["backup_sha256"] not in sources
                    and capture["backup_sha256"] not in backup_ids
                    and capture["receipt_sha256"] not in receipts and capture["storage_sha256"] not in copies,
                    "Backup capture cadence, independent copy or bytes differ")
            receipts.add(capture["receipt_sha256"])
            copies.add(capture["storage_sha256"])
            backup_ids.add(capture["backup_sha256"])
            recorded = {"backup_sha256": capture["backup_sha256"], "backup_storage_sha256": capture["storage_sha256"],
                        "backup_domain_sha256": capture["domain_sha256"], "checkpoint_sha256": capture["checkpoint_sha256"],
                        "captured_at": capture["captured_at"], "completed_at": capture["finished_at"],
                        "source_storage_sha256": expected["source_storage_sha256"], "home_sha256": expected["home_sha256"],
                        "manifest_sha256": capture["manifest_sha256"]}
            known_copies[capture["storage_sha256"]] = known_copies[capture["backup_sha256"]] = recorded
            totals[kind] += capture["bytes"]
            previous = began
            previous_capture = captured
        last = captures[-1]
        require(last["backup_sha256"] == expected["backup_sha256"] and last["storage_sha256"] == expected["backup_storage_sha256"]
                and last["domain_sha256"] == expected["backup_domain_sha256"] and last["manifest_sha256"] == expected["manifest_sha256"],
                "Latest readable copy differs from reviewed home")
    require(not copies & backup_ids, "Backup and physical storage identities overlap")
    return {"copied_bytes": totals, "campaign_seconds": (finish - start).total_seconds()}, known_copies


def verify_commitments(scope, references, files):
    require(has_exact_fields(references, DEPENDENCIES), "Critical dependency evidence differs")
    for name, reference in references.items():
        require(reference["sha256"] == scope["commitments"][name], "Dependency evidence differs from review")
        row = files.json(reference)
        require(has_exact_fields(row, {"dependency", "approval_sha256", "support_sha256", "maintenance_sha256", "breaking_change_sha256",
                     "compatibility_sha256", "upgrade_contact_sha256", "sla_sha256", "no_sla_risk_sha256"}), "Dependency commitment is incomplete")
        require(row["dependency"] == name and hashes(row[key] for key in set(row) - {"dependency", "sla_sha256", "no_sla_risk_sha256"})
                and ((hash_value(row["sla_sha256"]) and row["no_sla_risk_sha256"] is None)
                     or (row["sla_sha256"] is None and hash_value(row["no_sla_risk_sha256"]))), "Dependency support or SLA risk is unresolved")


class RecoveryEvidence:
    def __init__(self, scope, files, historical_copies):
        self.scope, self.files = scope, files
        self.runs, self.faults, self.targets, self.observations = set(), set(), set(), set()
        self.loss_events = set()
        self.inventory = scope["inventory"]
        self.source_storage = {home["source_storage_sha256"] for home in self.inventory.values()}
        self.backup_storage = set(historical_copies)
        self.exercise_copies = dict(historical_copies)

    def artifact(self, reference):
        row = self.files.json(reference)
        require(reference["sha256"] not in self.observations, "Recovery observation was reused")
        self.observations.add(reference["sha256"])
        return row

    def execution(self, reference, name, inventory, regional):
        row = self.artifact(reference)
        require(has_exact_fields(row, {"case", "mode", "candidate_sha256", "run_id", "attempt", "cancelled", "status", "started_at", "finished_at",
                     "samples", "fault", "cut", "readback", "stages", "copies"}), "Recovery execution is incomplete")
        require(row["case"] == name and row["mode"] == self.scope["mode"] and row["candidate_sha256"] == sha256(self.scope["candidate"])
                and hash_value(row["run_id"]) and row["run_id"] not in self.runs and type(row["attempt"]) is int and row["attempt"] == 1
                and row["cancelled"] is False and row["status"] == "PASS" and type(row["samples"]) is int and row["samples"] == len(inventory),
                "Recovery is duplicated, incomplete or bound to another candidate")
        self.runs.add(row["run_id"])
        start, end = timestamp(row["started_at"]), timestamp(row["finished_at"])
        require(timestamp(self.scope["window"]["start"]) <= start < end <= timestamp(self.scope["window"]["end"]), "Recovery execution is outside its reviewed window")
        fault = self.artifact(row["fault"])
        require(has_exact_fields(fault, {"run_id", "fault_sha256", "case", "loss_at", "source_domain_sha256", "recovery_domain_sha256", "source_unavailable"}), "Independent failure observation is incomplete")
        loss = timestamp(fault["loss_at"])
        require(regional or self.scope["phase"] == "frozen_recheck" or end < datetime(2026, 11, 20, tzinfo=SGT),
                "Initial home sample missed its qualification date")
        loss_event = (fault["source_domain_sha256"], loss)
        require(fault["run_id"] == row["run_id"] and fault["case"] == name and hash_value(fault["fault_sha256"])
                and fault["fault_sha256"] not in self.faults and loss_event not in self.loss_events and start <= loss < end
                and fault["source_unavailable"] is True and hashes(fault[key] for key in ("source_domain_sha256", "recovery_domain_sha256"))
                and fault["source_domain_sha256"] != fault["recovery_domain_sha256"], "Independent fault was reused or did not isolate the source")
        self.faults.add(fault["fault_sha256"])
        self.loss_events.add(loss_event)
        if regional:
            reviewed = self.scope["failure_scopes"][name]
            require(all(fault[key] == reviewed[key] for key in ("source_domain_sha256", "recovery_domain_sha256")), "Fault scope differs from review")
        else:
            require(fault["source_domain_sha256"] == self.inventory[name]["source_domain_sha256"], "Sample fault scope differs from home source")
        cut = self.artifact(row["cut"])
        require(has_exact_fields(cut, {"run_id", "fault_sha256", "mongo_at", "common_at", "checkpoint_sha256", "data_classes", "home_backups", "mongo_point"}), "Consistent cut is incomplete")
        require(cut["run_id"] == row["run_id"] and cut["fault_sha256"] == fault["fault_sha256"] and hash_value(cut["checkpoint_sha256"]), "Consistent cut identity differs")
        mongo, common = timestamp(cut["mongo_at"]), timestamp(cut["common_at"])
        require(common <= mongo <= loss and loss - mongo <= timedelta(minutes=5) and loss - common <= timedelta(hours=1), "Recovery point objective exceeded")
        require(has_exact_fields(cut["data_classes"], CUSTODY), "Consistent cut class inventory differs")
        for data_class, item in cut["data_classes"].items():
            require(has_exact_fields(item, {"checkpoint_sha256", "available_at", "independent_copy_sha256", "copy_storage_sha256",
                          "source_storage_sha256", "copy_domain_sha256"}), "Consistent cut class is missing its copy")
            require(item["checkpoint_sha256"] == cut["checkpoint_sha256"] and common <= timestamp(item["available_at"]) <= loss
                    and hashes(item[key] for key in ("independent_copy_sha256", "copy_storage_sha256", "source_storage_sha256", "copy_domain_sha256"))
                    and item["copy_domain_sha256"] != fault["source_domain_sha256"], "Consistent data-class cut differs")
            self.source_storage.add(item["source_storage_sha256"])
            copy_identity = {"data_class": data_class, **{key: item[key] for key in
                             ("independent_copy_sha256", "copy_storage_sha256", "source_storage_sha256", "copy_domain_sha256")}}
            for identity in (item["independent_copy_sha256"], item["copy_storage_sha256"]):
                require(identity not in self.source_storage | self.targets
                        and self.exercise_copies.setdefault(identity, copy_identity) == copy_identity,
                        "Data-class backup aliases another storage identity")
                self.backup_storage.add(identity)
        mongo_point = self.artifact(cut["mongo_point"])
        require(has_exact_fields(mongo_point, {"run_id", "fault_sha256", "copy_sha256", "earliest_at", "latest_at", "recoverable_at", "observed_at"}), "Independent Mongo recovery point is incomplete")
        mongo_copy = cut["data_classes"]["mongo"]
        earliest, latest = timestamp(mongo_point["earliest_at"]), timestamp(mongo_point["latest_at"])
        require(mongo_point["run_id"] == row["run_id"] and mongo_point["fault_sha256"] == fault["fault_sha256"]
                and mongo_point["copy_sha256"] == mongo_copy["independent_copy_sha256"]
                and earliest <= common <= mongo == timestamp(mongo_point["recoverable_at"]) <= latest <= loss
                and latest <= timestamp(mongo_copy["available_at"]) <= loss
                and timestamp(mongo_copy["available_at"]) <= timestamp(mongo_point["observed_at"]) <= end,
                "Mongo recoverable point is outside the independently observed PITR copy")
        copies = self.artifact(row["copies"])
        require(row["copies"]["sha256"] == self.scope["exercise_copy_receipts"][name], "Exercise copy observation differs from review")
        require(has_exact_fields(copies, {"run_id", "fault_sha256", "candidate_sha256", "homes"}), "Exercise backup observation is incomplete")
        require(copies["run_id"] == row["run_id"] and copies["fault_sha256"] == fault["fault_sha256"]
                and copies["candidate_sha256"] == row["candidate_sha256"], "Exercise backup observation differs from run")
        require(has_exact_fields(copies["homes"], inventory), "Exercise backup observation omits a reviewed home")
        require(has_exact_fields(cut["home_backups"], inventory), "Consistent cut omits a reviewed home")
        for home, backup in cut["home_backups"].items():
            expected = self.inventory[home]
            copy = copies["homes"][home]
            require(has_exact_fields(copy, {"backup_sha256", "backup_storage_sha256", "backup_domain_sha256", "checkpoint_sha256",
                          "captured_at", "completed_at", "source_storage_sha256", "home_sha256", "manifest_sha256"}), "Exercise copy identity is incomplete")
            require(all(copy[key] == expected[key] for key in ("source_storage_sha256", "home_sha256", "manifest_sha256"))
                    and hashes(copy[key] for key in ("backup_sha256", "backup_storage_sha256", "backup_domain_sha256"))
                    and copy["backup_sha256"] != copy["backup_storage_sha256"]
                    and not {copy["backup_sha256"], copy["backup_storage_sha256"]} & self.source_storage
                    and copy["backup_domain_sha256"] != expected["source_domain_sha256"], "Exercise copy is not independently retained")
            for key in ("backup_sha256", "backup_storage_sha256"):
                identity = copy[key]
                require(self.exercise_copies.setdefault(identity, copy) == copy, "Exercise copy identity was reused for another observation")
                self.backup_storage.add(identity)
            require(has_exact_fields(backup, {"backup_sha256", "backup_storage_sha256", "backup_domain_sha256", "checkpoint_sha256", "available_at"}), "Home backup cut is incomplete")
            require(all(backup[key] == copy[key] for key in ("backup_sha256", "backup_storage_sha256", "backup_domain_sha256"))
                    and backup["backup_domain_sha256"] != fault["source_domain_sha256"]
                    and backup["checkpoint_sha256"] == cut["checkpoint_sha256"] == copy["checkpoint_sha256"]
                    and common == timestamp(copy["captured_at"])
                    and common < timestamp(backup["available_at"]) == timestamp(copy["completed_at"]) <= loss
                    and timestamp(copy["completed_at"]) - common <= timedelta(minutes=30),
                    "Recovery cut uses an unavailable or inconsistent home backup")
        readback = self.artifact(row["readback"])
        require(has_exact_fields(readback, {"run_id", "fault_sha256", "checkpoint_sha256", "mongo_restored_at", "service_verified_at", "homes", "old_writer_fenced",
                          "tombstones_replayed", "key_custody_verified", "object_inventory_verified", "duplicate_effects",
                          "unreadable_records", "secret_matches", "resurrected_records", "mongo_recoverable_at", "mongo_copy_sha256"}), "Recovery readback is incomplete")
        require(readback["run_id"] == row["run_id"] and readback["fault_sha256"] == fault["fault_sha256"]
                and readback["checkpoint_sha256"] == cut["checkpoint_sha256"]
                and timestamp(readback["mongo_recoverable_at"]) == mongo
                and readback["mongo_copy_sha256"] == mongo_point["copy_sha256"] and all(readback[key] is True for key in
                ("old_writer_fenced", "tombstones_replayed", "key_custody_verified", "object_inventory_verified"))
                and all(type(readback[key]) is int and readback[key] == 0 for key in
                        ("duplicate_effects", "unreadable_records", "secret_matches", "resurrected_records")), "Recovered custody or effect ledger differs")
        mongo_restored, verified = timestamp(readback["mongo_restored_at"]), timestamp(readback["service_verified_at"])
        require(loss <= mongo_restored <= verified <= end and verified - loss <= timedelta(hours=4 if regional else 0, minutes=0 if regional else 30), "Recovery time objective exceeded")
        require(has_exact_fields(readback["homes"], inventory), "Restored home inventory differs")
        home_times = []
        home_starts, home_finishes = [], []
        for key, actual in readback["homes"].items():
            expected = self.inventory[key]
            identity = set(expected) - {"class", "source_storage_sha256", "source_domain_sha256"}
            require(has_exact_fields(actual, identity | {"restored_storage_sha256", "restored_domain_sha256", "started_at", "verified_at"}), "Restored home identity is incomplete")
            expected = {**expected, **{field: copies["homes"][key][field] for field in
                                      ("backup_sha256", "backup_storage_sha256", "backup_domain_sha256")}}
            require(all(actual[field] == expected[field] for field in identity)
                    and actual["restored_domain_sha256"] == fault["recovery_domain_sha256"]
                    and hash_value(actual["restored_storage_sha256"]) and actual["restored_storage_sha256"] not in self.source_storage | self.backup_storage | self.targets,
                    "Restored home content, used bytes, identity or independent storage differs")
            self.targets.add(actual["restored_storage_sha256"])
            home_time = timestamp(actual["verified_at"])
            home_start = timestamp(actual["started_at"])
            require(loss <= home_start < home_time <= verified, "Home verification is outside the recovery window")
            home_starts.append(home_start)
            home_finishes.append(home_time)
            home_times.append((home_time - loss).total_seconds())
        if regional:
            intervals = sorted(zip(home_starts, home_finishes))
            require(any(next_start < finish for (_, finish), (next_start, _) in zip(intervals, intervals[1:])),
                    "Parallel home restoration is unproven")
            self.stage_order(row["stages"], row["run_id"], loss, verified, reviewed)
        else:
            require(row["stages"] is None, "Sample recovery has regional readiness evidence")
        return {"restored_homes": len(inventory), "used_bytes": sum(self.inventory[home]["used_bytes"] for home in inventory),
                "mongo_rpo_seconds": (loss - mongo).total_seconds(), "integrated_rpo_seconds": (loss - common).total_seconds(),
                "mongo_restore_seconds": (mongo_restored - loss).total_seconds(), "home_max_rto_seconds": max(home_times),
                "verified_service_seconds": (verified - loss).total_seconds(),
                "restore_bytes_per_second": sum(self.inventory[home]["used_bytes"] for home in inventory) /
                (max(home_finishes) - min(home_starts)).total_seconds()}

    def stage_order(self, reference, run_id, loss, verified, reviewed):
        row = self.artifact(reference)
        require(has_exact_fields(row, {"run_id", "stages", "consumers", "available_quota_bytes", "capacity_receipt_sha256",
                     "throttle_receipt_sha256", "queue_receipt_sha256", "manual_steps_receipt_sha256"}), "Regional readiness evidence is incomplete")
        require(row["run_id"] == run_id and type(row["available_quota_bytes"]) is int and row["available_quota_bytes"] >= reviewed["quota_bytes"]
                and hashes(row[key] for key in ("capacity_receipt_sha256", "throttle_receipt_sha256", "queue_receipt_sha256", "manual_steps_receipt_sha256"))
                and isinstance(row["stages"], list) and len(row["stages"]) == len(STAGES), "Regional recovery capacity differs")
        last = loss
        for name, stage in zip(STAGES, row["stages"]):
            require(has_exact_fields(stage, {"stage", "verified_at", "receipt_sha256"}), "Regional readiness stage is incomplete")
            observed = timestamp(stage["verified_at"])
            require(stage["stage"] == name and hash_value(stage["receipt_sha256"]) and last <= observed <= verified, "Regional readiness order differs")
            last = observed
        require(row["consumers"] == reviewed["consumer_order"] and last == verified, "Consumer priority or service verification differs")


def validate_initial_result(result, homes, used_bytes, limit):
    bounds = {"mongo_rpo_seconds": 300, "integrated_rpo_seconds": 3600, "mongo_restore_seconds": limit,
              "home_max_rto_seconds": limit, "verified_service_seconds": limit}
    require(has_exact_fields(result, {"restored_homes", "used_bytes", "restore_bytes_per_second", *bounds}), "Initial recovery result differs")
    require(result["restored_homes"] == homes and result["used_bytes"] == used_bytes
            and all(type(result[key]) in (int, float) and 0 <= result[key] <= upper for key, upper in bounds.items())
            and type(result["restore_bytes_per_second"]) in (int, float) and result["restore_bytes_per_second"] >= 0,
            "Initial recovery cohort or objective differs")


def verify(scope_bytes, expected_hash, index, files):
    require(hash_value(expected_hash) and digest(scope_bytes) == expected_hash, "Scope differs from independent hash")
    scope = decode(scope_bytes)
    validate_scope(scope)
    require(has_exact_fields(index, {"scope_sha256", "coverage", "backup_campaign", "commitments", "regional", "home_sample", "initial_report"}), "Observation index is incomplete")
    require(index["scope_sha256"] == expected_hash, "Observation index binds another scope")
    verify_coverage(scope, index["coverage"], files)
    backup, historical_copies = verify_backup_campaign(scope, index["backup_campaign"], files)
    verify_commitments(scope, index["commitments"], files)
    require(has_exact_fields(index["regional"], SCOPES), "Regional fault observations are incomplete")
    require(has_exact_fields(index["home_sample"], scope["home_sample"]), "Independent home sample is incomplete")
    evidence = RecoveryEvidence(scope, files, historical_copies)
    regional = {name: evidence.execution(reference, name, scope["inventory"], True) for name, reference in index["regional"].items()}
    sample = {key: evidence.execution(reference, key, {key}, False) for key, reference in index["home_sample"].items()}
    require(not evidence.targets & (evidence.backup_storage | evidence.source_storage)
            and not evidence.source_storage & evidence.backup_storage,
            "A source, backup or recovery destination aliases another storage identity")
    candidate_sha = sha256(scope["candidate"])
    inventory_sha = sha256({key: {name: value for name, value in home.items() if not name.startswith("backup_")}
                            for key, home in scope["inventory"].items()})
    plan_sha = sha256({key: scope[key] for key in ("failure_scopes", "home_sample", "copy_changes")})
    if scope["phase"] == "initial":
        require(index["initial_report"] is None, "Initial qualification references a recheck")
    else:
        reference = index["initial_report"]
        require(reference["sha256"] == scope["initial_report_sha256"], "Initial report differs from review")
        initial = files.json(reference)
        require(initial.get("schema") == "cma-regional-recovery-report-v1" and initial.get("status") == "CONSISTENT"
                and initial.get("phase") == "initial" and initial.get("mode") == scope["mode"]
                and initial.get("candidate_sha256") == candidate_sha and initial.get("inventory_sha256") == inventory_sha
                and initial.get("commitments_sha256") == sha256(scope["commitments"]) and initial.get("plan_sha256") == plan_sha, "Initial report does not bind this recovery scope")
        require(has_exact_fields(initial.get("regional"), SCOPES), "Initial regional result inventory differs")
        require(has_exact_fields(initial.get("sample"), scope["home_sample"]), "Initial home sample result inventory differs")
        for result in initial["regional"].values():
            validate_initial_result(result, len(scope["inventory"]), sum(home["used_bytes"] for home in scope["inventory"].values()), 14400)
        for key, result in initial["sample"].items():
            validate_initial_result(result, 1, scope["inventory"][key]["used_bytes"], 1800)
        backup_result = initial.get("backup_campaign")
        require(has_exact_fields(backup_result, {"copied_bytes", "campaign_seconds"}), "Initial backup campaign result differs")
        require(backup_result["copied_bytes"] == backup["copied_bytes"]
                and type(backup_result["campaign_seconds"]) in (int, float) and backup_result["campaign_seconds"] > 0,
                "Initial backup campaign is incomplete")
        for name, current in (("runs", evidence.runs), ("faults", evidence.faults), ("restored_storage", evidence.targets)):
            previous = initial.get(name)
            require(isinstance(previous, list) and hashes(previous) and len(previous) == len(set(previous)) == len(current)
                    and not current & set(previous), "Frozen recheck reused initial evidence")
    return {"schema": "cma-regional-recovery-report-v1", "status": "CONSISTENT", "phase": scope["phase"], "mode": scope["mode"],
            "candidate_sha256": candidate_sha, "scope_sha256": expected_hash, "inventory_sha256": inventory_sha, "plan_sha256": plan_sha,
            "commitments_sha256": sha256(scope["commitments"]), "backup_campaign": backup, "regional": regional, "sample": sample,
            "runs": sorted(evidence.runs), "faults": sorted(evidence.faults), "restored_storage": sorted(evidence.targets),
            "observation_sha256": sha256(index)}


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
        result = verify(read_file(args.scope), args.scope_sha256, decode(read_file(args.observation)), EvidenceFiles(args.observation.parent))
        fd = os.open(output, os.O_WRONLY | os.O_CREAT | os.O_EXCL | os.O_NOFOLLOW, 0o600)
        with os.fdopen(fd, "w", encoding="utf-8") as stream:
            stream.write(canonical(result) + "\n")
        print("Reviewed recovery evidence is consistent")
        return 0
    except (OSError, ValueError, TypeError, KeyError, AttributeError):
        print("Recovery evidence is incomplete or inconsistent; raw diagnostics withheld", file=sys.stderr)
        return 1


if __name__ == "__main__":
    raise SystemExit(main())
