"""Data erasure, consent and export evidence checks; no runtime writes."""
from datetime import timedelta

from data_qualification_cases import DATA_CLASSES, ENTRIES, quantity
from release_candidate_evidence import has_exact_fields, hash_value, timestamp
from staging_cd_common import require

STAGES = ("sandbox_retire", "pvc_delete", "mongo_delete", "document_delete", "key_revocation")
LIVE_CLASSES = DATA_CLASSES - {"backups"}


def between(value, start, finish):
    return start <= timestamp(value) <= finish


def no_residue(value, classes):
    require(has_exact_fields(value, classes), "Data class reconciliation is incomplete")
    require(all(isinstance(items, list) and not items for items in value.values()),
            "Reconciliation found retained resources or access")


def check_deletion(cases, inventory, policy):
    expected = inventory["deletion"]
    physical = cases.case("TC-136", expected)
    restores = cases.case("TC-065", expected)
    all_audits = set()
    retried = False
    independent_inventories = set()
    for owner, before in expected.items():
        row = physical[owner]
        data = row["data"]
        require(has_exact_fields(data, {"owner_sha256", "home_state", "sandbox_ids", "inventory_before", "physical", "actual_entry", "requested_at",
                      "admitted_at", "revoked_at", "old_access", "duplicate_request_effects", "stages", "live_after",
                      "independent_inventory", "other_owner_changes", "backup_policy"}), "Physical deletion receipt is malformed")
        require(row["samples"] == 1 and data["owner_sha256"] == owner
                and data["home_state"] == before["home_state"] and data["sandbox_ids"] == before["sandbox_ids"]
                and data["inventory_before"] == before["classes"]
                and data["physical"] is True and data["actual_entry"] is True, "Deletion scope or physical reconciliation differs")
        independent = cases.files.json(data["independent_inventory"])
        require(has_exact_fields(independent, {"owner_sha256", "sandbox_ids", "sandbox_after", "inventory_before", "live_after", "observed_at"}), "Independent deletion inventory is malformed")
        require(independent["owner_sha256"] == owner and independent["sandbox_ids"] == before["sandbox_ids"]
                and independent["sandbox_after"] == [] and independent["inventory_before"] == before["classes"]
                and independent["live_after"] == data["live_after"]
                and data["independent_inventory"]["sha256"] not in independent_inventories,
                "Independent deletion reconciliation differs or was reused")
        independent_inventories.add(data["independent_inventory"]["sha256"])
        start, end = timestamp(row["started_at"]), timestamp(row["finished_at"])
        request, admission, revoked = map(timestamp, (data["requested_at"], data["admitted_at"], data["revoked_at"]))
        require(start <= request <= revoked <= admission <= end and end - request <= timedelta(days=7)
                and type(data["duplicate_request_effects"]) is int and data["duplicate_request_effects"] == 0,
                "Deletion missed the access or live data deadline")
        access = data["old_access"]
        require(has_exact_fields(access, {"old_identity", "share_link", "scoped_key"}), "Revocation observations are incomplete")
        for observation in access.values():
            require(has_exact_fields(observation, LIVE_CLASSES), "Revocation data surfaces differ")
            require(all(value is True for value in observation.values()), "Old access survived admission")
        require(type(data["other_owner_changes"]) is int and data["other_owner_changes"] == 0,
                "Deletion affected unrelated data")
        require(isinstance(data["stages"], list) and len(data["stages"]) == len(STAGES),
                "Deletion cascade is incomplete")
        last = admission
        clearances = {}
        stage_classes = dict(zip(STAGES[1:], ("home", "mongo", "document_objects", "credentials")))
        expected_stages = [before["sandbox_ids"], before["classes"]["home"], before["classes"]["mongo"],
                           before["classes"]["document_objects"], before["classes"]["credentials"]]
        for stage, observation, resources in zip(STAGES, data["stages"], expected_stages):
            require(has_exact_fields(observation, {"stage", "started_at", "finished_at", "attempts", "audit",
                                 "settled"}), "Deletion audit is malformed")
            audit = cases.files.json(observation["audit"])
            require(has_exact_fields(audit, {"owner_sha256", "stage", "started_at", "finished_at", "resources", "remaining",
                           "settled"}), "Independent deletion audit is malformed")
            require(audit["owner_sha256"] == owner and audit["resources"] == resources and audit["remaining"] == []
                    and all(audit[key] == observation[key] for key in ("stage", "started_at", "finished_at", "settled")),
                    "Deletion stage does not reconcile reviewed resources")
            stage_start, stage_end = timestamp(observation["started_at"]), timestamp(observation["finished_at"])
            require(observation["stage"] == stage and last <= stage_start <= stage_end <= end
                    and type(observation["attempts"]) is int and observation["attempts"] > 0
                    and observation["audit"]["sha256"] not in all_audits
                    and observation["settled"] is True, "Deletion cascade order, audit or settlement differs")
            all_audits.add(observation["audit"]["sha256"])
            retried = retried or observation["attempts"] > 1
            last = stage_end
            if stage in stage_classes:
                clearances[stage_classes[stage]] = stage_end
        require(between(independent["observed_at"], last, end), "Independent deletion inventory predates settlement")
        clearances["logs"] = timestamp(independent["observed_at"])
        for data_class, cleared in clearances.items():
            retention = policy["data_classes"][data_class]["retention_seconds"]
            require(cleared - request <= timedelta(seconds=retention), "Live data clearance exceeded approved retention")
        no_residue(data["live_after"], LIVE_CLASSES)
        backup = policy["data_classes"]["backups"]
        require(has_exact_fields(data["backup_policy"], before["classes"]["backups"]), "Backup copy inventory differs")
        for copy in data["backup_policy"].values():
            require(has_exact_fields(copy, {"retention_seconds", "policy_sha256", "exception_approval_sha256", "observed_real_policy"}), "Backup expiry evidence is malformed")
            require(copy["retention_seconds"] == backup["retention_seconds"]
                    and copy["exception_approval_sha256"] == backup["exception_approval_sha256"]
                    and hash_value(copy["policy_sha256"]) and copy["observed_real_policy"] is True,
                    "Actual backup retention policy differs")
        restore = restores[owner]
        result = restore["data"]
        require(has_exact_fields(result, {"owner_sha256", "backups", "independent_restore", "restore_execution", "clock_mode", "tombstone_before_serving",
                        "restored_after", "residual_access_count", "backup_policy", "deletion_receipt_sha256",
                        "restore_environment_sha256"}), "Deletion marker restore evidence is malformed")
        ref = cases.index["cases"]["TC-136"][owner]
        require(restore["samples"] == len(before["classes"]["backups"])
                and result["owner_sha256"] == owner and result["backups"] == before["classes"]["backups"]
                and result["independent_restore"] is True and result["restore_execution"] == "physical"
                and hash_value(result["restore_environment_sha256"])
                and result["restore_environment_sha256"] != restore["environment_sha256"]
                and timestamp(restore["started_at"]) >= end
                and result["clock_mode"] in ("real", "accelerated_fixture")
                and result["tombstone_before_serving"] is True
                and type(result["residual_access_count"]) is int and result["residual_access_count"] == 0
                and result["deletion_receipt_sha256"] == ref["sha256"]
                and result["backup_policy"] == data["backup_policy"],
                "Restore does not prove the original deletion and actual backup policy")
        no_residue(result["restored_after"], DATA_CLASSES)
    require(retried, "Deletion cohort did not exercise retry")


def check_delivery(data, expected, policy, start, finish):
    require(has_exact_fields(data, {"requested_at", "delivered_at", "manifest", "delivery", "archive_readable",
                  "destination_authorized", "document_export", "scope_sha256", "third_party_fields",
                  "raw_credentials", "expired_link_access", "revoked_link_access", "wrong_identity_access",
                  "paused_notice"}), "Export observation is malformed")
    requested, delivered = timestamp(data["requested_at"]), timestamp(data["delivered_at"])
    export = policy["export"]
    require(start <= requested <= delivered <= finish and
            delivered - requested <= timedelta(seconds=export["delivery_seconds"])
            and data["scope_sha256"] == export["approval_sha256"],
            "Export delivery exceeded the approved scope or deadline")
    require(data["delivery"] == expected["delivery"] and data["archive_readable"] is True
            and data["destination_authorized"] is True and data["document_export"] is True
            and data["manifest"] == expected["classes"], "Export manifest or delivery scope differs")
    require(all(type(data[name]) is int and data[name] == 0 for name in
                ("third_party_fields", "raw_credentials", "expired_link_access", "revoked_link_access",
                 "wrong_identity_access")), "Export contains unauthorized content or access")
    if expected["home_state"] == "active":
        require(data["paused_notice"] is None, "Active home has a paused retention observation")
        return
    notice = data["paused_notice"]
    require(has_exact_fields(notice, {"policy_sha256", "paused_at", "expires_at", "notice_at", "delivered", "export_available"}), "Paused home notice is incomplete")
    approved = policy["paused_home"]
    paused, expires, notified = map(timestamp, (notice["paused_at"], notice["expires_at"], notice["notice_at"]))
    require(notice["policy_sha256"] == approved["approval_sha256"] and notice["delivered"] is True
            and notice["export_available"] is True and expires - paused == timedelta(seconds=approved["retention_seconds"])
            and paused <= notified <= expires - timedelta(seconds=approved["notice_seconds"])
            and notified <= finish, "Paused home retention or advance notice differs")


def check_notice_entries(value, policy, expected_version):
    require(has_exact_fields(value, ENTRIES), "All three legal entry points are required")
    for entry in value.values():
        require(has_exact_fields(entry, {"anonymous", "authenticated"}), "Legal entry point access inventory differs")
        for observation in entry.values():
            require(has_exact_fields(observation, {"terms_version", "privacy_version", "notice_sha256", "terms_http", "privacy_http"}), "Displayed legal notice is malformed")
            require(observation["terms_version"] == observation["privacy_version"]
                    and observation["terms_version"] == expected_version
                    and observation["notice_sha256"] == policy["legal"]["notices"][expected_version]
                    and observation["terms_http"] == observation["privacy_http"] == 200,
                    "Displayed notice differs from approved legal identity")


def check_export(cases, inventory, policy):
    expected = inventory["exports"]
    exports = cases.case("TC-137", expected)
    observed_manifests = set()
    for owner, definition in expected.items():
        row = exports[owner]
        data = row["data"]
        require(has_exact_fields(data, {"owner_sha256", "home_state", "actual_entry", "result", "independent_manifest"}), "Account export is malformed")
        require(data["owner_sha256"] == owner and data["home_state"] == definition["home_state"]
                and data["actual_entry"] is True and row["samples"] == 1, "Account export identity differs")
        check_delivery(data["result"], definition, policy,
                       timestamp(row["started_at"]), timestamp(row["finished_at"]))
        manifest = cases.files.json(data["independent_manifest"])
        require(has_exact_fields(manifest, {"owner_sha256", "delivery", "manifest", "observed_at"}), "Independent export manifest is malformed")
        require(manifest["owner_sha256"] == owner and manifest["delivery"] == definition["delivery"]
                and manifest["manifest"] == definition["classes"]
                and between(manifest["observed_at"], timestamp(data["result"]["delivered_at"]), timestamp(row["finished_at"]))
                and data["independent_manifest"]["sha256"] not in observed_manifests,
                "Independent export manifest differs or was reused")
        observed_manifests.add(data["independent_manifest"]["sha256"])
    notice = cases.case("TC-119", set(inventory["notice_accounts"]))
    for owner, row in notice.items():
        data = row["data"]
        require(has_exact_fields(data, {"owner_sha256", "actual_entry", "legal_approval_sha256", "scope_sha256",
                      "manual_review_sha256", "export_receipt_sha256", "entries", "disclosure_matches"}), "Manual export and consumer notice review is incomplete")
        require(data["owner_sha256"] == owner and row["samples"] == 1 and data["actual_entry"] is True
                and data["legal_approval_sha256"] == policy["legal"]["approval_sha256"]
                and data["scope_sha256"] == policy["export"]["approval_sha256"]
                and hash_value(data["manual_review_sha256"]) and data["disclosure_matches"] is True
                and data["export_receipt_sha256"] == cases.index["cases"]["TC-137"][owner]["sha256"],
                "Manual notice review does not bind its actual export")
        check_notice_entries(data["entries"], policy, policy["legal"]["versions"][-1])


def check_consent(cases, inventory, policy):
    expected = inventory["consents"]
    cohort = set(expected["successful"]) | set(expected["declined"]) | set(expected["failed"])
    rows = cases.case("TC-135", cohort | {"entry-points"})
    entry = rows["entry-points"]
    require(has_exact_fields(entry["data"], {"versions", "approval_sha256", "previous_versions_retained"}), "Consent entry point evidence is incomplete")
    require(entry["data"]["approval_sha256"] == policy["legal"]["approval_sha256"]
            and entry["data"]["previous_versions_retained"] is True
            and entry["samples"] == len(ENTRIES) * 4 * len(policy["legal"]["versions"]),
            "Legal notice approval or immutable version history differs")
    require(has_exact_fields(entry["data"]["versions"], policy["legal"]["versions"]), "Consent entry point version inventory differs")
    for version, entries in entry["data"]["versions"].items():
        check_notice_entries(entries, policy, version)
    for owner in cohort:
        row = rows[owner]
        data = row["data"]
        require(has_exact_fields(data, {"owner_sha256", "registration", "displayed_version", "consent_at", "registration_at",
                      "before_count", "after_count", "retained_consent_sha256", "readback_consent_sha256"}), "Registration consent observation is malformed")
        require(data["owner_sha256"] == owner and row["samples"] == 1
                and quantity(data["before_count"]) and quantity(data["after_count"]) and data["displayed_version"] in policy["legal"]["versions"],
                "Registration consent identity differs")
        if owner in expected["successful"]:
            require(data["registration"] == "succeeded" and data["displayed_version"] == expected["successful"][owner]
                    and data["after_count"] == data["before_count"] + 1
                    and timestamp(row["started_at"]) <= timestamp(data["consent_at"]) <= timestamp(data["registration_at"])
                    <= timestamp(row["finished_at"]) and hash_value(data["retained_consent_sha256"])
                    and data["retained_consent_sha256"] == data["readback_consent_sha256"],
                    "Successful registration lacks immutable displayed consent")
        else:
            require(timestamp(row["started_at"]) <= timestamp(data["registration_at"]) <= timestamp(row["finished_at"]),
                    "Registration attempt falls outside its observation window")
            require(data["registration"] == ("declined" if owner in expected["declined"] else "failed")
                    and data["after_count"] == data["before_count"] and data["consent_at"] is None
                    and data["retained_consent_sha256"] is None and data["readback_consent_sha256"] is None,
                    "Declined or failed registration produced a false consent")
