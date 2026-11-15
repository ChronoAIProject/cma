"""Pure event/custody/effect accounting; caller binds independent finality receipts.

This reports evidence categories, never an architect or production PASS. Final
process identity, log completeness and effect finality require reviewed outer
receipts. A supplied boolean cannot establish the underlying fact by itself.
"""

from observations import checked_hash


def require(condition, code):
    if not condition:
        raise ValueError(code)


def accepted(row):
    return (
        row.get("status") == 202
        and row.get("accepted_body") is True
        and row.get("body_complete") is True
    )


def _pre_submit_refusal(requests, observations):
    """Every observed root write must have its actual pre-submit fault marker."""
    for request in requests:
        if request.get("status") != 503 or request.get("body_complete") is not True:
            return False
        joined = [
            row
            for row in observations
            if row.get("request_sha256") == request["request_sha256"]
        ]
        roots = {
            row["attempt_sha256"]
            for row in joined
            if row.get("event") == "eg_admission_stage"
            and row.get("boundary") == "start"
            and row.get("operations")
            and row["operations"][-1]["stage"] == "root_write"
        }
        markers = {
            row["attempt_sha256"]
            for row in joined
            if row.get("event") == "eg_fixture_record_fault"
            and row.get("boundary") == "before_submission"
            and row.get("commit_class") == "not_submitted"
        }
        if not roots or roots != markers:
            return False
        for row in joined:
            if (
                row.get("event") == "eg_admission_record"
                and row.get("attempt_sha256") in roots
            ):
                if row.get("phase") in {"record_update", "record_insert", "commit"}:
                    return False
    return bool(requests)


def _joined(requests, observations, admission):
    if not requests or not admission:
        return False
    for request in requests:
        rows = [
            row
            for row in observations
            if row.get("request_sha256") == request["request_sha256"]
            and row.get("event") == "eg_admission_attempt"
        ]
        if not rows or any(row.get("admission_sha256") != admission for row in rows):
            return False
        starts = {
            (row.get("process_sha256"), row.get("attempt_sha256"))
            for row in rows
            if row.get("boundary") == "start"
        }
        ends = {
            (row.get("process_sha256"), row.get("attempt_sha256"))
            for row in rows
            if row.get("boundary") in {"end", "cancelled"}
        }
        all_identities = {
            (row.get("process_sha256"), row.get("attempt_sha256"))
            for row in observations
            if row.get("request_sha256") == request["request_sha256"]
            and row.get("attempt_sha256") is not None
        }
        if (
            not starts
            or starts != ends
            or starts != all_identities
            or any(None in identity for identity in starts)
        ):
            return False
    return True


def account(registry, cases, final, captures, finality):
    """Registry is the pre-registered offered event set, not successes discovered later.

    Registry rows have event_sha256 and expect=success|pre_submit_refusal. The
    source-reviewed driver supports exactly one routed target and text effect.
    Case receipts retain every HTTP attempt, including failed/timeout attempts.
    Missing case results do not remove their event from the denominator.
    """
    require(
        isinstance(registry, list) and 0 < len(registry) <= 8, "invalid_event_registry"
    )
    events = {}
    for row in registry:
        require(set(row) == {"event_sha256", "expect"}, "invalid_event_registration")
        key = checked_hash(row["event_sha256"])
        require(
            key not in events and row["expect"] in {"success", "pre_submit_refusal"},
            "invalid_event_registration",
        )
        events[key] = row["expect"]
    require(
        final.get("schema") == "eg_fixture_reconciliation_v1", "invalid_final_oracle"
    )
    require(
        set(finality)
        == {
            "receipt_sha256",
            "all_gateway_processes_exited",
            "no_late_effect_barrier",
            "capture_complete",
        },
        "invalid_finality",
    )
    checked_hash(finality["receipt_sha256"])
    require(
        all(type(finality[k]) is bool for k in finality if k != "receipt_sha256"),
        "invalid_finality_flag",
    )
    requests = {key: [] for key in events}
    direct = set()
    seen_cases, seen_requests = set(), set()
    for case in cases:
        require(case.get("schema") == "eg_fixture_case_v1", "invalid_case")
        key = case["event_sha256"]
        require(key in events, "unregistered_case_event")
        case_hash = checked_hash(case["case_sha256"])
        require(case_hash not in seen_cases, "duplicate_case_receipt")
        seen_cases.add(case_hash)
        for request in case["requests"]:
            request_hash = checked_hash(request["request_sha256"])
            require(request_hash not in seen_requests, "duplicate_http_attempt")
            seen_requests.add(request_hash)
            requests[key].append(request)
        if (
            _complete(case.get("durable", {}))
            and all(accepted(r) for r in case["requests"])
            and case["requests"]
        ):
            direct.add(key)
    final_events = {}
    for row in final["events"]:
        key = checked_hash(row["event_sha256"])
        require(key in events and key not in final_events, "invalid_final_event_set")
        final_events[key] = row
    ledger = final["ledger"]
    ledger_ok = (
        ledger.get("closed") is True
        and ledger.get("poisoned") is False
        and ledger.get("journal_attached") is True
        and ledger.get("rejected_requests") == 0
    )
    observations = [row for packet in captures for row in packet["observations"]]
    capture_ok = bool(captures) and all(
        packet.get("rejected_records") == 0 for packet in captures
    )
    finality_ok = (
        ledger_ok
        and capture_ok
        and all(v for k, v in finality.items() if k != "receipt_sha256")
    )
    for row in ledger["provider_operations"] + ledger["sink_effects"]:
        require(row["event_sha256"] in events, "unregistered_external_effect")
    counts = {
        name: 0
        for name in (
            "http_attempts",
            "submitted_events",
            "proven_unadmitted",
            "custody_established",
            "admission_uncertain",
            "direct_terminal",
            "reconciled_terminal",
            "unresolved",
            "terminal_failures",
            "expected_effects",
            "confirmed_effects",
            "duplicate_effects",
            "missing_effects",
            "stale_effects",
            "wrong_owner_effects",
        )
    }
    counts["http_attempts"] = len(seen_requests)
    counts["submitted_events"] = len(events)
    rows, contracts = [], True
    invalid_effects = 0
    for key, expect in events.items():
        durable = final_events.get(key, {})
        providers = [
            r for r in ledger["provider_operations"] if r["event_sha256"] == key
        ]
        effects = [r for r in ledger["sink_effects"] if r["event_sha256"] == key]
        targets = durable.get("targets", [])
        custody = durable.get("root_present") is True
        matching = (
            len(targets) == len(providers) == len(effects) == 1
            and providers[0]["idempotency_sha256"] == targets[0]["idempotency_sha256"]
            and effects[0]["text_matches"] is True
            and effects[0]["after_close"] is False
        )
        counts["duplicate_effects"] += max(0, len(effects) - 1) + max(
            0, len(providers) - 1
        )
        counts["stale_effects"] += sum(r["after_close"] is True for r in effects)
        invalid_effects += sum(r["text_matches"] is not True for r in effects)
        if (
            providers
            and targets
            and {r["idempotency_sha256"] for r in providers}
            != {r["idempotency_sha256"] for r in targets}
        ):
            invalid_effects += 1
        joined = _joined(requests[key], observations, durable.get("admission_sha256"))
        terminal = (
            custody and _complete(durable) and matching and finality_ok and joined
        )
        if custody:
            counts["custody_established"] += 1
            counts["expected_effects"] += 1
            counts["missing_effects"] += int(not effects)
            if terminal:
                bucket = "direct_terminal" if key in direct else "reconciled_terminal"
                counts[bucket] += 1
                counts["confirmed_effects"] += 1
            else:
                bucket = "unresolved"
                counts[bucket] += 1
        elif (
            durable.get("root_present") is False
            and not providers
            and not effects
            and finality_ok
            and joined
            and _pre_submit_refusal(requests[key], observations)
        ):
            bucket = "proven_unadmitted"
            counts[bucket] += 1
        else:
            bucket = "admission_uncertain"
            counts[bucket] += 1
        contract = (
            bool(requests[key]) and all(accepted(r) for r in requests[key]) and terminal
            if expect == "success"
            else bucket == "proven_unadmitted"
        )
        contracts = contracts and contract
        rows.append(
            {
                "event_sha256": key,
                "http_attempts": len(requests[key]),
                "bucket": bucket,
                "expected_http_contract_met": contract,
                "provider_operations": len(providers),
                "sink_effects": len(effects),
            }
        )
    complete = (
        contracts
        and finality_ok
        and invalid_effects == 0
        and not any(
            counts[k]
            for k in (
                "admission_uncertain",
                "unresolved",
                "duplicate_effects",
                "missing_effects",
                "stale_effects",
                "wrong_owner_effects",
            )
        )
    )
    return {
        "schema": "eg_event_accounting_v1",
        "counts": counts,
        "events": rows,
        "invalid_or_unjoined_effects": invalid_effects,
        "evidence_status": "COMPLETE" if complete else "INCOMPLETE",
        "application_acceptance": "REQUIRES_INDEPENDENT_REVIEW",
        "finality_receipt_sha256": finality["receipt_sha256"],
    }


def _complete(durable):
    targets = durable.get("targets")
    return (
        durable.get("root_present") is True
        and isinstance(targets, list)
        and len(targets) == 1
        and targets[0].get("complete") is True
    )
