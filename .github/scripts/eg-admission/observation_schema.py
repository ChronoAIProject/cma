"""Closed vocabularies for the admission diagnostic, never arbitrary log data."""

STAGES = frozenset(
    "root_lookup chunk_write chunk_read root_write root_readback enqueue index_retire".split()
)
CLASSES = frozenset(
    "pending ok refused invalid too_large conflict guard_conflict expired expired_or_unavailable unavailable uncertain corrupt missing_chunk not_found rejected malformed".split()
)
PUBLIC_CODES = frozenset(
    "invalid_request not_authenticated forbidden channel_not_found channel_restore_unavailable channel_invalid provider_unavailable provider_rejected provider_busy draining event_ref_invalid event_ref_expired callback_invalid admission_refused rate_limited conflict payload_too_large unsupported_media_type internal github_signature_invalid github_installation_forbidden github_resource_not_found github_delivery_conflict github_source_unavailable github_dispatch_unavailable github_integration_retired github_proof_withdrawn github_delivery_expired attachment_not_found media_unavailable".split()
)
PUBLIC_MAPPING = {
    "conflict": (409, "conflict", "routed_event_conflict"),
    "guard_conflict": (503, "provider_unavailable", "routed_authority_changed_retry"),
    "too_large": (413, "payload_too_large", "routed_event_too_large"),
    "invalid": (400, "invalid_request", "routed_event_invalid"),
    "expired": (410, "event_ref_expired", "routed_event_expired"),
    **{
        key: (503, "provider_unavailable", "routed_admission_unavailable")
        for key in "expired_or_unavailable unavailable uncertain corrupt missing_chunk".split()
    },
}
RECORD_ENUMS = {
    "record_operation": frozenset(("replica_write", "admission_index")),
    "phase": frozenset(
        "validate session transaction_start channel_guard partition_guard record_update record_insert commit backoff index_update index_legacy_read".split()
    ),
    "io_class": frozenset("local read transaction_write write commit".split()),
    "boundary": frozenset("start error end deadline cancelled".split()),
    "outcome": frozenset("pending ok error deadline cancelled".split()),
    "error_class": frozenset(
        "none unknown_commit transient_transaction unlabelled_mongo not_found conflict guard_conflict rejected unavailable malformed deadline cancelled".split()
    ),
    "label_class": frozenset(
        "not_observed unknown_commit transient_transaction unlabelled_mongo".split()
    ),
    "commit_class": frozenset("unconfirmed unknown aborted acknowledged".split()),
}
RECORD_BOOLEANS = frozenset(
    ("mongo_labels_observed", "transient_transaction_label", "unknown_commit_label")
)
IO_CLASS = {
    **{
        phase: "local"
        for phase in ("validate", "session", "transaction_start", "backoff")
    },
    **{
        phase: "transaction_write"
        for phase in (
            "channel_guard",
            "partition_guard",
            "record_update",
            "record_insert",
        )
    },
    "commit": "commit",
    "index_update": "write",
    "index_legacy_read": "read",
}


def exact_keys(value, keys):
    if set(value) != set(keys):
        raise ValueError("unexpected observation fields")


def integer(value, lower, upper):
    if type(value) is not int or not lower <= value <= upper:
        raise ValueError("invalid bounded observation number")


def validate_event(event):
    kind = event.get("event")
    if kind in ("eg_admission_attempt", "eg_admission_stage"):
        exact_keys(event, ("event", "boundary", "class", "elapsed_ms"))
        if (
            event["boundary"] not in ("start", "end", "cancelled")
            or event["class"] not in CLASSES
        ):
            raise ValueError("invalid stage classification")
        if event["boundary"] == "start" and event["class"] != "pending":
            raise ValueError("invalid stage start")
        if event["boundary"] == "cancelled" and event["class"] != "uncertain":
            raise ValueError("invalid cancellation classification")
        integer(event["elapsed_ms"], 0, 86_400_000)
        if event["boundary"] == "end" and event["class"] == "pending":
            raise ValueError("completed stage cannot remain pending")
    elif kind == "eg_admission_public_mapping":
        exact_keys(
            event, ("event", "class", "public_status", "public_code", "public_detail")
        )
        actual = (event["public_status"], event["public_code"], event["public_detail"])
        if PUBLIC_MAPPING.get(event["class"]) != actual:
            raise ValueError("invalid public mapping")
    elif kind == "eg_admission_response":
        exact_keys(event, ("event", "public_status", "public_code"))
        integer(event["public_status"], 400, 599)
        if event["public_code"] not in PUBLIC_CODES:
            raise ValueError("invalid public code")
    elif kind == "eg_admission_record":
        exact_keys(
            event,
            {"event", "record_operation_sequence", "transaction_attempt", "elapsed_ms"}
            | set(RECORD_ENUMS)
            | RECORD_BOOLEANS,
        )
        for key, allowed in RECORD_ENUMS.items():
            if event[key] not in allowed:
                raise ValueError("invalid record classification")
        for key in RECORD_BOOLEANS:
            if type(event[key]) is not bool:
                raise ValueError("invalid record label flag")
        integer(event["transaction_attempt"], 0, 5)
        integer(event["record_operation_sequence"], 1, 2**64 - 1)
        integer(event["elapsed_ms"], 0, 86_400_000)
        if event["io_class"] != IO_CLASS[event["phase"]]:
            raise ValueError("inconsistent client I/O classification")
        boundary = event["boundary"]
        expected_outcome = {
            "start": "pending",
            "error": "error",
            "deadline": "deadline",
            "cancelled": "cancelled",
        }.get(boundary, "ok" if event["error_class"] == "none" else "error")
        if event["outcome"] != expected_outcome:
            raise ValueError("inconsistent operation outcome")
        if boundary == "start" and (event["error_class"], event["commit_class"]) != (
            "none",
            "unconfirmed",
        ):
            raise ValueError("invalid operation start")
        if boundary in ("deadline", "cancelled") and (
            event["error_class"],
            event["commit_class"],
        ) != (boundary, "unknown"):
            raise ValueError("invalid interrupted operation")
        if boundary == "end" and event["commit_class"] != (
            "acknowledged" if event["error_class"] == "none" else "unknown"
        ):
            raise ValueError("invalid final operation acknowledgement")
        if event["mongo_labels_observed"]:
            label = (
                "unknown_commit"
                if event["unknown_commit_label"]
                else "transient_transaction"
                if event["transient_transaction_label"]
                else "unlabelled_mongo"
            )
            if (
                event["boundary"] != "error"
                or event["label_class"] != label
                or event["error_class"] != label
            ):
                raise ValueError("inconsistent observed labels")
            if event["unknown_commit_label"] and event["commit_class"] != "unknown":
                raise ValueError("unknown commit was downgraded")
            expected_commit = (
                "aborted"
                if label == "transient_transaction"
                and event["record_operation"] == "replica_write"
                else "unknown"
            )
            if event["commit_class"] != expected_commit:
                raise ValueError("inconsistent transaction classification")
        elif (
            event["label_class"] != "not_observed"
            or event["unknown_commit_label"]
            or event["transient_transaction_label"]
        ):
            raise ValueError("unobserved labels cannot be inferred")
    else:
        raise ValueError("unknown observation event")
