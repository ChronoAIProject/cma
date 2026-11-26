"""Quota, key recovery and reviewed SU-DATA inventory checks."""
from release_candidate_evidence import has_exact_fields, hash_value, timestamp
from staging_cd_common import require

DOCUMENT_LIMIT = 100
BYTE_LIMIT = 1024 ** 3
DATA_CLASSES = {"mongo", "home", "document_objects", "credentials", "logs", "backups"}
EXPORT_CLASSES = {"mongo", "home", "history", "documents", "credentials", "logs", "backups"}
KEY_CLASSES = {"oauth_token", "api_bearer", "actor_grant", "gateway_sealed"}
KEY_STAGES = {"interrupted_rotation", "coexisting_keys", "independent_restore"}
ENTRIES = {"landing", "signup", "account"}
RECONCILIATION = {"failed_upload", "retry", "delete"}


def quantity(value):
    return type(value) is int and value >= 0


def hashes(values, minimum=1):
    require(isinstance(values, list) and len(values) >= minimum
            and all(hash_value(value) for value in values) and len(set(values)) == len(values),
            "Reviewed resource inventory is incomplete or duplicated")


def keyed(value, minimum=1):
    require(isinstance(value, dict) and len(value) >= minimum and all(hash_value(key) for key in value),
            "Reviewed account or operation inventory is incomplete")


def positive(value):
    return type(value) is int and value > 0


def approved_policy(policy):
    require(has_exact_fields(policy, {"quota", "data_classes", "legal", "export", "paused_home", "key_custody_sha256"}), "Reviewed policy inventory is incomplete")
    quota = policy["quota"]
    require(has_exact_fields(quota, {"approval_sha256", "documents", "retained_bytes"}), "Document quota approval is missing")
    require(hash_value(quota["approval_sha256"]) and quota["documents"] == DOCUMENT_LIMIT
            and quota["retained_bytes"] == BYTE_LIMIT, "Approved document quota differs")
    require(has_exact_fields(policy["data_classes"], DATA_CLASSES), "Data class policy is incomplete")
    for name, row in policy["data_classes"].items():
        require(has_exact_fields(row, {"owner_sha256", "retention_seconds", "exception_approval_sha256"}), "Retention policy is malformed")
        require(hash_value(row["owner_sha256"]) and positive(row["retention_seconds"]),
                "Data class custody or retention is unapproved")
        limit = 30 * 86400 if name == "backups" else 7 * 86400
        approval = row["exception_approval_sha256"]
        require((approval is None and row["retention_seconds"] <= limit)
                or (name == "backups" and hash_value(approval)), "Retention exception is unapproved")
    legal = policy["legal"]
    require(has_exact_fields(legal, {"approval_sha256", "notices", "versions"}), "Legal notice approval is incomplete")
    require(hash_value(legal["approval_sha256"]),
            "Legal notice approval is incomplete")
    hashes(legal["versions"], 2)
    require(has_exact_fields(legal["notices"], legal["versions"]), "Reviewed legal version notice inventory differs")
    require(all(hash_value(notice) for notice in legal["notices"].values()), "Legal notice identity is malformed")
    export = policy["export"]
    require(has_exact_fields(export, {"approval_sha256", "delivery_seconds", "classes"}), "Export policy is incomplete")
    require(hash_value(export["approval_sha256"]) and positive(export["delivery_seconds"]), "Export policy is unapproved")
    require(has_exact_fields(export["classes"], EXPORT_CLASSES), "Approved export class inventory is incomplete")
    for name, row in export["classes"].items():
        require(has_exact_fields(row, {"included", "format_or_reason_sha256"}), "Export class decision is malformed")
        require(type(row["included"]) is bool and hash_value(row["format_or_reason_sha256"]),
                "Export class decision is unapproved")
    require(all(export["classes"][name]["included"] is True for name in ("home", "history", "documents"))
            and export["classes"]["credentials"]["included"] is False, "Required export scope differs")
    paused = policy["paused_home"]
    require(has_exact_fields(paused, {"approval_sha256", "retention_seconds", "notice_seconds"}), "Paused home policy is undecided")
    require(hash_value(paused["approval_sha256"]) and positive(paused["retention_seconds"])
            and positive(paused["notice_seconds"]) and paused["notice_seconds"] < paused["retention_seconds"],
            "Paused home policy is undecided")
    require(hash_value(policy["key_custody_sha256"]), "Independent key custody is unapproved")


def validate_inventory(inventory, policy):
    approved_policy(policy)
    require(has_exact_fields(inventory, {"quota", "keys", "deletion", "exports", "notice_accounts", "consents"}), "Reviewed data inventory is incomplete")
    quota = inventory["quota"]
    require(has_exact_fields(quota, {"count_operations", "byte_operations", "reconciliation", "legacy_owner"}), "Quota inventory is incomplete")
    for name in ("count_operations", "byte_operations"):
        rows = quota[name]
        keyed(rows, 20)
        require(len(rows) == 20 and all(positive(value) for value in rows.values()), "Concurrent quota denominator differs")
    require(not set(quota["count_operations"]) & set(quota["byte_operations"]), "Quota operations were reused")
    keyed(quota["reconciliation"], 100)
    require(len(quota["reconciliation"]) == 100, "Reconciliation denominator differs")
    for row in quota["reconciliation"].values():
        require(has_exact_fields(row, {"kind", "expected_sha256"}), "Reconciliation inventory is malformed")
        require(row["kind"] in RECONCILIATION and hash_value(row["expected_sha256"]), "Reconciliation identity differs")
    require({row["kind"] for row in quota["reconciliation"].values()} == RECONCILIATION
            and hash_value(quota["legacy_owner"]), "Quota scenario inventory is incomplete")
    require(has_exact_fields(inventory["keys"], KEY_CLASSES), "Key record inventory is incomplete")
    for values in inventory["keys"].values():
        hashes(values)
    keyed(inventory["deletion"], 10)
    delete_resources = set()
    for row in inventory["deletion"].values():
        require(has_exact_fields(row, {"home_state", "sandbox_ids", "classes"}), "Deletion inventory is malformed")
        hashes(row["sandbox_ids"])
        require(not delete_resources & set(row["sandbox_ids"]), "Sandbox inventory was reused")
        delete_resources.update(row["sandbox_ids"])
        require(row["home_state"] in ("active", "paused"), "Deletion home state is unreviewed")
        require(has_exact_fields(row["classes"], DATA_CLASSES), "Deletion data class inventory is incomplete")
        for values in row["classes"].values():
            hashes(values)
            require(not delete_resources & set(values), "Deletion resource was assigned to multiple inventory rows")
            delete_resources.update(values)
    require({row["home_state"] for row in inventory["deletion"].values()} == {"active", "paused"},
            "Deletion cohort must include paused and active homes")
    keyed(inventory["exports"], 10)
    export_resources = set()
    for row in inventory["exports"].values():
        require(has_exact_fields(row, {"home_state", "delivery", "classes"}), "Export inventory is malformed")
        require(row["home_state"] in ("active", "paused") and row["delivery"] in ("download", "github"),
                "Export scope is malformed")
        included = {key for key, value in policy["export"]["classes"].items() if value["included"]}
        require(has_exact_fields(row["classes"], included), "Export inventory differs from approved scope")
        for items in row["classes"].values():
            keyed(items)
            require(not export_resources & set(items), "Export inventory reused a resource across rows")
            export_resources.update(items)
            for item in items.values():
                require(has_exact_fields(item, {"bytes", "sha256"}), "Export item identity is malformed")
                require(quantity(item["bytes"]) and hash_value(item["sha256"]), "Export item identity is malformed")
    require({row["delivery"] for row in inventory["exports"].values()} == {"download", "github"}
            and {row["home_state"] for row in inventory["exports"].values()} == {"active", "paused"},
            "Export cohort does not cover both paths and retained homes")
    hashes(inventory["notice_accounts"], 3)
    require(set(inventory["notice_accounts"]) <= set(inventory["exports"]), "Notice account inventory is unbound")
    consents = inventory["consents"]
    require(has_exact_fields(consents, {"successful", "declined", "failed"}), "Consent inventory is incomplete")
    keyed(consents["successful"], 10)
    require(set(consents["successful"].values()) == set(policy["legal"]["versions"]), "Consent version inventory differs")
    hashes(consents["declined"])
    hashes(consents["failed"])
    require(not (set(consents["successful"]) & set(consents["declined"])
                 or set(consents["successful"]) & set(consents["failed"])
                 or set(consents["declined"]) & set(consents["failed"])), "Consent accounts overlap")


def boundary(row, operations, count_boundary):
    data = row["data"]
    require(has_exact_fields(data, {"before_documents", "before_bytes", "peak_documents", "peak_bytes",
                  "after_documents", "after_bytes", "operations"}), "Quota boundary evidence is malformed")
    require(has_exact_fields(data["operations"], operations), "Concurrent operations differ from reviewed inventory")
    for name in set(data) - {"operations"}:
        require(quantity(data[name]), "Quota observation is malformed")
    before_docs, before_bytes = data["before_documents"], data["before_bytes"]
    require(before_docs < DOCUMENT_LIMIT and before_bytes < BYTE_LIMIT, "Quota boundary baseline is invalid")
    if count_boundary:
        require(before_docs == DOCUMENT_LIMIT - 1, "Document count boundary was not exercised")
    else:
        require(0 < BYTE_LIMIT - before_bytes < sum(operations.values()), "Retained bytes boundary was not exercised")
    require(row["samples"] == len(operations), "Concurrent quota sample count differs")
    starts, finishes, accepted = [], [], []
    for name, observed in data["operations"].items():
        require(has_exact_fields(observed, {"started_at", "finished_at", "outcome", "typed_refusal", "effects"}), "Concurrent operation outcome is malformed")
        start, finish = timestamp(observed["started_at"]), timestamp(observed["finished_at"])
        require(timestamp(row["started_at"]) <= start < finish <= timestamp(row["finished_at"]),
                "Concurrent operation lies outside its observation window")
        starts.append(start)
        finishes.append(finish)
        if observed["outcome"] == "accepted":
            require(type(observed["effects"]) is int and observed["effects"] == 1
                    and observed["typed_refusal"] is False, "Accepted quota operation effect differs")
            accepted.append(name)
        else:
            require(observed["outcome"] == "refused" and observed["typed_refusal"] is True
                    and type(observed["effects"]) is int and observed["effects"] == 0,
                    "Quota refusal occurred after an effect")
    require(max(starts) < min(finishes) and 0 < len(accepted) < len(operations),
            "Concurrent quota acceptance and refusal are unproven")
    expected_docs = before_docs + (len(accepted) if count_boundary else 0)
    expected_bytes = before_bytes + sum(operations[name] for name in accepted)
    require(data["after_documents"] == expected_docs and data["after_bytes"] == expected_bytes
            and expected_docs <= data["peak_documents"] <= DOCUMENT_LIMIT
            and expected_bytes <= data["peak_bytes"] <= BYTE_LIMIT, "Quota inventory overshot or did not reconcile")


def check_quota(cases, inventory, policy):
    quota = inventory["quota"]
    rows = cases.case("TC-063", {"count", "bytes", "reconciliation", "legacy"})
    boundary(rows["count"], quota["count_operations"], True)
    boundary(rows["bytes"], quota["byte_operations"], False)
    reconciled = rows["reconciliation"]
    require(reconciled["samples"] == len(quota["reconciliation"]), "Reconciliation sample count differs")
    require(has_exact_fields(reconciled["data"], quota["reconciliation"]), "Reconciliation receipt inventory differs")
    for name, expected in quota["reconciliation"].items():
        row = reconciled["data"][name]
        require(has_exact_fields(row, {"kind", "state_sha256", "reservation_count", "orphan_count", "duplicate_effects",
                     "failed_upload_reservation_released"}), "Reconciliation receipt is malformed")
        require(row["kind"] == expected["kind"] and row["state_sha256"] == expected["expected_sha256"]
                and all(type(row[key]) is int and row[key] == 0 for key in
                        ("reservation_count", "orphan_count", "duplicate_effects"))
                and row["failed_upload_reservation_released"] is True, "Quota reconciliation failed")
    legacy = rows["legacy"]["data"]
    require(has_exact_fields(legacy, {"owner_sha256", "documents", "bytes", "read_hash_before", "read_hash_after",
                    "typed_refusal", "effects"}), "Historical over-quota evidence is malformed")
    require(legacy["owner_sha256"] == quota["legacy_owner"]
            and quantity(legacy["documents"]) and quantity(legacy["bytes"])
            and (legacy["documents"] > DOCUMENT_LIMIT or legacy["bytes"] > BYTE_LIMIT)
            and hash_value(legacy["read_hash_before"]) and legacy["read_hash_after"] == legacy["read_hash_before"]
            and legacy["typed_refusal"] is True and type(legacy["effects"]) is int and legacy["effects"] == 0,
            "Historical over-quota data was not preserved")


def check_key_recovery(cases, inventory, policy):
    names = {kind + "-" + stage for kind in KEY_CLASSES for stage in KEY_STAGES}
    rows = cases.case("TC-064", names)
    for kind, identities in inventory["keys"].items():
        for stage in KEY_STAGES:
            row = rows[kind + "-" + stage]
            data = row["data"]
            require(has_exact_fields(data, {"record_class", "stage", "records", "custody_sha256", "independent_restore",
                          "restore_environment_sha256", "scan_files", "scan_matches"}), "Key recovery observation is malformed")
            require(data["record_class"] == kind and data["stage"] == stage
                    and data["custody_sha256"] == policy["key_custody_sha256"]
                    and type(data["independent_restore"]) is bool
                    and ((stage == "independent_restore" and data["independent_restore"]
                          and hash_value(data["restore_environment_sha256"])
                          and data["restore_environment_sha256"] != row["environment_sha256"])
                         or (stage != "independent_restore" and not data["independent_restore"]
                             and data["restore_environment_sha256"] is None))
                    and positive(data["scan_files"]) and type(data["scan_matches"]) is int and data["scan_matches"] == 0
                    and row["samples"] == len(identities), "Key recovery scope or scan differs")
            require(has_exact_fields(data["records"], identities), "Key recovery omitted a retained record")
            for record in data["records"].values():
                require(has_exact_fields(record, {"valid_read", "invalid_key_refused", "wrong_deployment_refused"}), "Key recovery observation is malformed")
                require(record["valid_read"] is True and record["invalid_key_refused"] is True
                        and record["wrong_deployment_refused"] is True,
                        "Key recovery readability or refusal failed")
