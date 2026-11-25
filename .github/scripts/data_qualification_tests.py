"""Synthetic inputs exercise the seven-case SU-DATA evidence verifier."""
from datetime import datetime, timedelta, timezone
import json
from pathlib import Path
import tempfile
import unittest

from data_qualification import CASES, RECHECK_CUTOFFS, main, verify
from data_qualification_cases import DATA_CLASSES, ENTRIES, EXPORT_CLASSES, KEY_CLASSES, KEY_STAGES, RECONCILIATION
from data_qualification_privacy import LIVE_CLASSES, STAGES
from release_candidate_packet import EvidenceFiles, digest
from staging_cd_common import Refusal
from staging_cd_receipts import canonical, sha256
from staging_cd_targets import TARGETS


def stamp(minutes):
    return (datetime(2026, 11, 15, tzinfo=timezone.utc) + timedelta(minutes=minutes)).isoformat(
        timespec="milliseconds").replace("+00:00", "Z")


def hashes(prefix, count):
    return [sha256(prefix + str(i)) for i in range(count)]


def no_items(classes):
    return {name: [] for name in sorted(classes)}


class Campaign:
    def __init__(self, root, recheck=False, source="a" * 40, deletion_delay=0, recheck_day_offset=0):
        self.root = root
        self.offset = 0
        self.deletion_delay = deletion_delay
        self.root.mkdir(exist_ok=True)
        self.evidence = root / "evidence"
        self.evidence.mkdir()
        self.candidate = {
            "source": source, "images": {key: "sha256:" + sha256(key) for key in TARGETS},
            "configuration_sha256": sha256("config"), "flags_sha256": sha256("flags"),
            "environment_sha256": sha256("environment"),
        }
        included = {"mongo", "home", "history", "documents"}
        self.policy = {
            "quota": {"approval_sha256": sha256("approved-quota"), "documents": 100, "retained_bytes": 1024 ** 3},
            "data_classes": {name: {"owner_sha256": sha256("owner-" + name),
                                   "retention_seconds": 30 * 86400 if name == "backups" else 7 * 86400,
                                   "exception_approval_sha256": None} for name in DATA_CLASSES},
            "legal": {"approval_sha256": sha256("legal-approval"), "versions": hashes("version", 2),
                      "notices": {version: sha256("notice-" + version) for version in hashes("version", 2)}},
            "export": {"approval_sha256": sha256("export-approval"), "delivery_seconds": 3600,
                       "classes": {name: {"included": name in included,
                                           "format_or_reason_sha256": sha256("format-" + name)}
                                   for name in EXPORT_CLASSES}},
            "paused_home": {"approval_sha256": sha256("pause-policy"), "retention_seconds": 30 * 86400,
                            "notice_seconds": 3 * 86400},
            "key_custody_sha256": sha256("key-custody"),
        }
        deletion, exports = {}, {}
        for i, owner in enumerate(hashes("delete-owner", 10)):
            deletion[owner] = {"home_state": "active" if i % 2 else "paused",
                               "sandbox_ids": hashes(owner + "sandbox", 1),
                               "classes": {name: hashes(owner + name, 1) for name in DATA_CLASSES}}
        for i, owner in enumerate(hashes("export-owner", 10)):
            exports[owner] = {"home_state": "paused" if i % 2 else "active",
                              "delivery": "download" if i % 2 else "github",
                              "classes": {name: {sha256(owner + name): {"bytes": 10, "sha256": sha256(name)}}
                                          for name in included}}
        quota = {"count_operations": {key: 16 for key in hashes("count-op", 20)},
                 "byte_operations": {key: 16 for key in hashes("byte-op", 20)},
                 "reconciliation": {key: {"kind": sorted(RECONCILIATION)[i % len(RECONCILIATION)],
                                          "expected_sha256": sha256(key)} for i, key in enumerate(hashes("recon", 100))},
                 "legacy_owner": sha256("historical-owner")}
        self.inventory = {
            "quota": quota, "keys": {kind: hashes(kind, 2) for kind in KEY_CLASSES},
            "deletion": deletion, "exports": exports, "notice_accounts": list(exports)[:3],
            "consents": {"successful": {key: self.policy["legal"]["versions"][i % 2]
                                        for i, key in enumerate(hashes("registered", 10))},
                         "declined": hashes("declined-registration", 1), "failed": hashes("failed-registration", 1)},
        }
        self.scope = {"schema": "cma-data-scope-v1", "mode": "synthetic", "candidate": self.candidate,
                      "policy": self.policy, "inventory": self.inventory,
                      "phase": "frozen_recheck" if recheck else "initial",
                      "windows": {case: {"start": stamp(0), "end": stamp(60), "inputs": {}} for case in CASES},
                      "frozen_at": stamp(8 * 1440) if recheck else None, "initial_report_sha256": None}
        self.index = {"schema": "cma-data-observations-v1", "scope_sha256": "",
                      "cases": {case: {} for case in CASES}, "initial_report": None}
        if recheck:
            self.scope["windows"] = {
                case: {"start": stamp((day - 15 + recheck_day_offset) * 1440),
                       "end": stamp((day - 15 + recheck_day_offset) * 1440 + 60), "inputs": {}}
                for case, day in RECHECK_CUTOFFS.items()
            }
            self.index["cases"] = {case: {} for case in RECHECK_CUTOFFS}
            self.offset = (RECHECK_CUTOFFS["TC-064"] - 15 + recheck_day_offset) * 1440
            self.seed_keys()
            self.seed_deletion()
            self.offset = (RECHECK_CUTOFFS["TC-119"] - 15 + recheck_day_offset) * 1440
            self.seed_exports()
        else:
            for case in ("TC-065", "TC-136"):
                self.scope["windows"][case]["end"] = stamp(60 + deletion_delay)
            self.seed_quota()
            self.seed_keys()
            self.seed_deletion()
            self.seed_exports()
            self.seed_consent()

    def stamp(self, minutes):
        return stamp(minutes + self.offset)

    def ref(self, relative, data):
        path = self.evidence / relative
        path.parent.mkdir(parents=True, exist_ok=True)
        raw = canonical(data).encode()
        path.write_bytes(raw)
        return {"path": relative, "sha256": digest(raw)}

    def read(self, reference):
        return json.loads((self.evidence / reference["path"]).read_bytes())

    def add(self, case, name, data, samples=1, start=0, finish=15):
        inputs = {"input_sha256": sha256("input-" + case + name),
                  "seed_sha256": sha256("seed-" + str(self.offset) + case + name)}
        if case in self.scope["windows"]:
            self.scope["windows"][case]["inputs"][name] = inputs
        row = {"case": case, "id": name, "candidate_sha256": sha256(self.candidate), **inputs,
               **{key: self.candidate[key] for key in ("configuration_sha256", "flags_sha256", "environment_sha256")},
               "run_id": sha256(str(self.offset) + case + name), "started_at": self.stamp(start), "finished_at": self.stamp(finish),
               "samples": samples, "attempt": 1, "cancelled": False, "status": "PASS",
               "mode": self.scope["mode"], "data": data}
        ref = self.ref(case + "/" + name + ".json", row)
        self.index["cases"][case][name] = ref
        return ref

    def replace(self, reference, change):
        row = self.read(reference)
        change(row)
        reference.update(self.ref(reference["path"], row))

    def seed_quota(self):
        quota = self.inventory["quota"]
        for name, expected in (("count", quota["count_operations"]), ("bytes", quota["byte_operations"])):
            before = 99 if name == "count" else 50
            before_bytes = 1000 if name == "count" else 1024 ** 3 - 16
            after = before + (name == "count")
            operations = {op: {"started_at": self.stamp(2), "finished_at": self.stamp(3),
                              "outcome": "accepted" if i == 0 else "refused",
                              "typed_refusal": i != 0, "effects": int(i == 0)}
                          for i, op in enumerate(expected)}
            self.add("TC-063", name, {"before_documents": before, "before_bytes": before_bytes,
                "peak_documents": after, "peak_bytes": before_bytes + 16, "after_documents": after,
                "after_bytes": before_bytes + 16, "operations": operations}, 20)
        self.add("TC-063", "reconciliation", {key: {"kind": value["kind"], "state_sha256": value["expected_sha256"],
            "reservation_count": 0, "orphan_count": 0, "duplicate_effects": 0,
            "failed_upload_reservation_released": True} for key, value in quota["reconciliation"].items()}, 100)
        self.add("TC-063", "legacy", {"owner_sha256": quota["legacy_owner"], "documents": 101, "bytes": 100,
            "read_hash_before": sha256("legacy"), "read_hash_after": sha256("legacy"), "typed_refusal": True, "effects": 0})

    def seed_keys(self):
        for kind, records in self.inventory["keys"].items():
            for stage in KEY_STAGES:
                independent = stage == "independent_restore"
                self.add("TC-064", kind + "-" + stage, {"record_class": kind, "stage": stage,
                    "records": {key: {"valid_read": True, "invalid_key_refused": True, "wrong_deployment_refused": True} for key in records},
                    "custody_sha256": self.policy["key_custody_sha256"], "independent_restore": independent,
                    "restore_environment_sha256": sha256("isolated-restore") if independent else None,
                    "scan_files": 4, "scan_matches": 0}, len(records))

    def seed_deletion(self):
        for owner, expected in self.inventory["deletion"].items():
            stages = []
            resources = [expected["sandbox_ids"], expected["classes"]["home"], expected["classes"]["mongo"],
                         expected["classes"]["document_objects"], expected["classes"]["credentials"]]
            for i, (stage, ids) in enumerate(zip(STAGES, resources)):
                stage_start, stage_end = self.stamp(2 + i + self.deletion_delay), self.stamp(3 + i + self.deletion_delay)
                audit = self.ref("audits/" + owner + "-" + stage + ".json", {"owner_sha256": owner,
                    "stage": stage, "started_at": stage_start, "finished_at": stage_end,
                    "resources": ids, "remaining": [], "settled": True})
                stages.append({"stage": stage, "started_at": stage_start, "finished_at": stage_end,
                               "attempts": 2 if i == 1 else 1, "audit": audit, "settled": True})
            independent = self.ref("inventories/" + owner + ".json", {"owner_sha256": owner,
                "sandbox_ids": expected["sandbox_ids"], "sandbox_after": [], "inventory_before": expected["classes"],
                "live_after": no_items(LIVE_CLASSES), "observed_at": self.stamp(8 + self.deletion_delay)})
            backup = {key: {"retention_seconds": self.policy["data_classes"]["backups"]["retention_seconds"],
                           "policy_sha256": sha256("backup-policy"), "exception_approval_sha256": None,
                           "observed_real_policy": True} for key in expected["classes"]["backups"]}
            deletion = self.add("TC-136", owner, {"owner_sha256": owner, "home_state": expected["home_state"],
                "sandbox_ids": expected["sandbox_ids"], "inventory_before": expected["classes"], "physical": True, "actual_entry": True,
                "requested_at": self.stamp(1), "admitted_at": self.stamp(2), "revoked_at": self.stamp(1.5),
                "old_access": {actor: {name: True for name in LIVE_CLASSES}
                               for actor in ("old_identity", "share_link", "scoped_key")},
                "duplicate_request_effects": 0, "stages": stages, "live_after": no_items(LIVE_CLASSES),
                "independent_inventory": independent, "other_owner_changes": 0, "backup_policy": backup}, finish=15 + self.deletion_delay)
            self.add("TC-065", owner, {"owner_sha256": owner, "backups": expected["classes"]["backups"],
                "independent_restore": True, "restore_execution": "physical",
                "restore_environment_sha256": sha256("isolated-restore"), "clock_mode": "accelerated_fixture",
                "tombstone_before_serving": True, "restored_after": no_items(DATA_CLASSES), "residual_access_count": 0,
                "backup_policy": backup, "deletion_receipt_sha256": deletion["sha256"]},
                len(expected["classes"]["backups"]), start=20 + self.deletion_delay, finish=30 + self.deletion_delay)

    def entries(self, version):
        return {name: {actor: {"terms_version": version, "privacy_version": version,
                               "notice_sha256": self.policy["legal"]["notices"][version], "terms_http": 200,
                               "privacy_http": 200} for actor in ("anonymous", "authenticated")} for name in ENTRIES}

    def seed_exports(self):
        for owner, expected in self.inventory["exports"].items():
            notice = ({"policy_sha256": self.policy["paused_home"]["approval_sha256"],
                       "paused_at": self.stamp(-10 * 1440), "expires_at": self.stamp(20 * 1440),
                       "notice_at": self.stamp(1), "delivered": True, "export_available": True}
                      if expected["home_state"] == "paused" else None)
            result = {"requested_at": self.stamp(2), "delivered_at": self.stamp(3), "manifest": expected["classes"],
                "delivery": expected["delivery"], "archive_readable": True, "destination_authorized": True,
                "document_export": True, "scope_sha256": self.policy["export"]["approval_sha256"],
                **{key: 0 for key in ("third_party_fields", "raw_credentials", "expired_link_access",
                                      "revoked_link_access", "wrong_identity_access")},
                "paused_notice": notice}
            own = self.ref("manifests/" + owner + ".json", {"owner_sha256": owner,
                "delivery": expected["delivery"], "manifest": expected["classes"], "observed_at": self.stamp(4)})
            self.add("TC-137", owner, {"owner_sha256": owner, "home_state": expected["home_state"],
                "actual_entry": True, "result": result, "independent_manifest": own})
        version = self.policy["legal"]["versions"][-1]
        for owner in self.inventory["notice_accounts"]:
            export = self.index["cases"]["TC-137"][owner]
            self.add("TC-119", owner, {"owner_sha256": owner, "actual_entry": True,
                "legal_approval_sha256": self.policy["legal"]["approval_sha256"],
                "scope_sha256": self.policy["export"]["approval_sha256"], "manual_review_sha256": sha256(owner),
                "export_receipt_sha256": export["sha256"], "entries": self.entries(version),
                "disclosure_matches": True})

    def seed_consent(self):
        expected = self.inventory["consents"]
        versions = self.policy["legal"]["versions"]
        self.add("TC-135", "entry-points", {"versions": {version: self.entries(version) for version in versions},
            "approval_sha256": self.policy["legal"]["approval_sha256"], "previous_versions_retained": True},
            len(ENTRIES) * 4 * len(versions))
        for owner, version in expected["successful"].items():
            receipt = sha256("consent-" + owner)
            self.add("TC-135", owner, {"owner_sha256": owner, "registration": "succeeded",
                "displayed_version": version, "consent_at": self.stamp(1), "registration_at": self.stamp(2),
                "before_count": 0, "after_count": 1, "retained_consent_sha256": receipt,
                "readback_consent_sha256": receipt})
        for kind in ("declined", "failed"):
            for owner in expected[kind]:
                self.add("TC-135", owner, {"owner_sha256": owner, "registration": kind,
                    "displayed_version": versions[-1], "consent_at": None, "registration_at": self.stamp(2),
                    "before_count": 0, "after_count": 0, "retained_consent_sha256": None, "readback_consent_sha256": None})

    def verify(self):
        raw = canonical(self.scope).encode()
        self.index["scope_sha256"] = digest(raw)
        return verify(raw, digest(raw), self.index, EvidenceFiles(self.evidence))

    def cli(self, override=None):
        raw = canonical(self.scope).encode()
        freeze = self.root / "scope.json"
        freeze.write_bytes(raw)
        self.index["scope_sha256"] = digest(raw)
        index = self.evidence / "index.json"
        index.write_text(canonical(self.index))
        output = self.root / "report.json"
        status = main(["--scope", str(freeze), "--scope-sha256", override or digest(raw),
                       "--observation", str(index), "--output", str(output)])
        return status, output


class QualificationTests(unittest.TestCase):
    def setUp(self):
        temp = tempfile.TemporaryDirectory()
        self.addCleanup(temp.cleanup)
        self.campaign = Campaign(Path(temp.name))

    def mutate(self, case, name, change):
        self.campaign.replace(self.campaign.index["cases"][case][name], change)
        with self.assertRaises(Refusal):
            self.campaign.verify()

    def test_complete_seven_case_synthetic_cli(self):
        status, output = self.campaign.cli()
        self.assertEqual(status, 0)
        result = json.loads(output.read_text())
        self.assertEqual(result["status"], "CONSISTENT")
        self.assertEqual(result["mode"], "synthetic")
        self.assertEqual(set(result["cases"]), CASES)
        self.assertEqual(result["cases"]["TC-136"], 10)
        self.assertEqual(result["cases"]["TC-137"], 10)
        self.assertEqual(output.stat().st_mode & 0o777, 0o600)

    def test_quota_is_concurrent_and_does_not_overshoot(self):
        self.mutate("TC-063", "count", lambda row: row["data"].update(peak_documents=101))

    def test_quota_missing_reconciliation_refuses(self):
        self.mutate("TC-063", "reconciliation", lambda row: row["data"].pop(next(iter(row["data"]))))

    def test_invalid_key_scope_and_readability_refuse(self):
        self.mutate("TC-064", "oauth_token-independent_restore", lambda row: row["data"].update(independent_restore=False))

    def test_deletion_independent_inventory_must_match(self):
        owner = next(iter(self.campaign.inventory["deletion"]))
        self.mutate("TC-136", owner, lambda row: row["data"]["live_after"]["logs"].append(sha256("residue")))

    def test_deletion_requires_actual_entry_and_all_revocation_surfaces(self):
        owner = next(iter(self.campaign.inventory["deletion"]))
        self.mutate("TC-136", owner, lambda row: row["data"]["old_access"]["share_link"].update(home=False))

    def test_legal_notice_is_bound_to_displayed_version(self):
        version = self.campaign.policy["legal"]["versions"][0]
        self.mutate("TC-135", "entry-points", lambda row: row["data"]["versions"][version]["signup"]["anonymous"].update(
            notice_sha256=self.campaign.policy["legal"]["notices"][self.campaign.policy["legal"]["versions"][1]]))

    def test_replayed_tombstone_cannot_be_omitted(self):
        owner = next(iter(self.campaign.inventory["deletion"]))
        self.mutate("TC-065", owner, lambda row: row["data"].update(tombstone_before_serving=False))

    def test_export_missing_item_and_unsigned_paused_policy_refuse(self):
        owner = next(iter(self.campaign.inventory["exports"]))
        self.mutate("TC-137", owner, lambda row: row["data"]["result"]["manifest"].pop("documents"))

    def test_undecided_paused_policy_refuses(self):
        self.campaign.policy["paused_home"]["approval_sha256"] = None
        with self.assertRaises(Refusal):
            self.campaign.verify()

    def test_consent_version_must_match_displayed_version(self):
        owner = next(iter(self.campaign.inventory["consents"]["successful"]))
        self.mutate("TC-135", owner, lambda row: row["data"].update(displayed_version=sha256("other-version")))

    def test_manual_notice_cannot_reference_other_exports(self):
        owner = self.campaign.inventory["notice_accounts"][0]
        self.mutate("TC-119", owner, lambda row: row["data"].update(export_receipt_sha256=sha256("other-export")))

    def test_accelerated_deletion_and_late_first_run_refuse(self):
        owner = next(iter(self.campaign.inventory["deletion"]))
        self.mutate("TC-136", owner, lambda row: row["data"].update(physical=False))

    def test_late_initial_qualification_refuses(self):
        owner = next(iter(self.campaign.inventory["deletion"]))
        self.mutate("TC-136", owner, lambda row: row.update(finished_at=stamp(6 * 1440)))

    def test_duplicate_run_and_cancelled_case_refuse(self):
        case = self.campaign.index["cases"]["TC-063"]
        duplicate = self.campaign.read(case["count"])["run_id"]
        self.mutate("TC-063", "bytes", lambda row: row.update(run_id=duplicate))

    def test_cancelled_case_refuses(self):
        self.mutate("TC-063", "legacy", lambda row: row.update(cancelled=True))

    def test_missing_data_class_refuses(self):
        owner = next(iter(self.campaign.inventory["deletion"]))
        self.campaign.inventory["deletion"][owner]["classes"].pop("logs")
        with self.assertRaises(Refusal):
            self.campaign.verify()

    def test_frozen_recheck_requires_initial_and_distinct_runs(self):
        initial = self.campaign.verify()
        recheck = Campaign(self.campaign.root / "recheck", recheck=True)
        with self.assertRaises(Refusal):
            recheck.verify()
        reference = recheck.ref("initial.json", initial)
        recheck.index["initial_report"] = reference
        recheck.scope["initial_report_sha256"] = reference["sha256"]
        result = recheck.verify()
        self.assertEqual(result["phase"], "frozen_recheck")
        self.assertEqual(set(result["cases"]), set(RECHECK_CUTOFFS))
        old_run = self.campaign.read(self.campaign.index["cases"]["TC-064"]["oauth_token-independent_restore"])["run_id"]
        recheck.replace(recheck.index["cases"]["TC-064"]["oauth_token-independent_restore"],
                        lambda row: row.update(run_id=old_run))
        with self.assertRaises(Refusal):
            recheck.verify()

    def test_frozen_recheck_rejects_another_candidate(self):
        other = Campaign(self.campaign.root / "other-candidate", source="b" * 40)
        initial = other.verify()
        recheck = Campaign(self.campaign.root / "recheck", recheck=True)
        reference = recheck.ref("initial.json", initial)
        recheck.index["initial_report"] = reference
        recheck.scope["initial_report_sha256"] = reference["sha256"]
        with self.assertRaises(Refusal):
            recheck.verify()

    def test_frozen_recheck_rejects_early_date(self):
        initial = self.campaign.verify()
        early = Campaign(self.campaign.root / "early-recheck", recheck=True, recheck_day_offset=-1)
        reference = early.ref("initial.json", initial)
        early.index["initial_report"] = reference
        early.scope["initial_report_sha256"] = reference["sha256"]
        with self.assertRaises(Refusal):
            early.verify()

    def test_wrong_deployment_must_be_refused(self):
        record = self.campaign.inventory["keys"]["oauth_token"][0]
        self.mutate("TC-064", "oauth_token-independent_restore", lambda row:
                    row["data"]["records"][record].update(wrong_deployment_refused=False))

    def test_live_class_clearance_must_meet_approved_retention(self):
        slow = Campaign(self.campaign.root / "slow-deletion", deletion_delay=2 * 1440)
        slow.verify()
        slow.policy["data_classes"]["home"]["retention_seconds"] = 86400
        with self.assertRaises(Refusal):
            slow.verify()
        slow.policy["data_classes"]["home"]["retention_seconds"] = 7 * 86400
        slow.policy["data_classes"]["logs"]["retention_seconds"] = 86400
        with self.assertRaises(Refusal):
            slow.verify()

    def test_receipt_with_changed_input_seed_refuses(self):
        self.mutate("TC-063", "legacy", lambda row: row.update(seed_sha256=sha256("unreviewed-seed")))

    def test_receipt_outside_reviewed_window_refuses(self):
        self.mutate("TC-063", "legacy", lambda row: row.update(finished_at=stamp(61)))

    def test_wrong_scope_hash_does_not_write_report(self):
        status, output = self.campaign.cli("0" * 64)
        self.assertEqual(status, 1)
        self.assertFalse(output.exists())


if __name__ == "__main__":
    unittest.main()
