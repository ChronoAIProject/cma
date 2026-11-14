"""Durable, allowlisted evidence for one owned EG cohort; no cluster mutations.

Receipts describe supplied observations. They do not independently certify the
inner oracle, container death, or application acceptance. A verifier must review
the referenced artifacts and exact execution binding.
"""
import datetime
import hashlib
import json
import math
import os
from pathlib import Path
import re
import time
import uuid


PHASES = (
    "prepare", "workload", "terminal_wait", "capture", "workload_shutdown",
    "resource_shutdown", "child_wait", "local_destroy", "cleanup", "transition",
)
BINDINGS = ("application_image", "namespace", "config", "fixture", "tools", "owner_window")
COUNTS = (
    "http_attempts", "submitted_events", "proven_unadmitted", "custody_established",
    "admission_uncertain", "direct_terminal", "reconciled_terminal", "unresolved",
    "terminal_failures", "expected_effects", "confirmed_effects", "duplicate_effects",
    "missing_effects", "stale_effects", "wrong_owner_effects",
)


class EvidenceError(ValueError):
    """Only fixed error codes may leave the evidence boundary."""


def require(condition, code):
    if not condition:
        raise EvidenceError(code)


def digest(value):
    require(isinstance(value, str) and re.fullmatch(r"[a-f0-9]{64}", value), "invalid_sha256")
    return value


def identifier(value):
    require(isinstance(value, str), "invalid_run_identity")
    try:
        parsed = uuid.UUID(value)
    except ValueError as error:
        raise EvidenceError("invalid_run_identity") from error
    require(parsed.version == 4 and str(parsed) == value, "invalid_run_identity")
    return value


def validate_plan(plan):
    require(isinstance(plan, dict) and set(plan) == {
        "schema", "run_id", "cohort_id", "replicas", "candidate_commit", "candidate_tree",
        "bindings", "budgets_ms", "expected_timeout_phase", "departing_instances",
    }, "invalid_plan_fields")
    require(plan["schema"] == "eg_lifecycle_plan_v1", "invalid_plan_schema")
    identifier(plan["run_id"])
    identifier(plan["cohort_id"])
    require(type(plan["replicas"]) is int and plan["replicas"] in (1, 2), "invalid_replica_count")
    for field in ("candidate_commit", "candidate_tree"):
        require(isinstance(plan[field], str) and re.fullmatch(r"[a-f0-9]{40}", plan[field]),
                "invalid_source_identity")
    require(isinstance(plan["bindings"], dict) and set(plan["bindings"]) == set(BINDINGS),
            "invalid_binding_fields")
    for value in plan["bindings"].values():
        digest(value)
    require(isinstance(plan["budgets_ms"], dict) and set(plan["budgets_ms"]) == set(PHASES),
            "invalid_phase_budgets")
    for value in plan["budgets_ms"].values():
        require(type(value) is int and 0 < value <= 120000, "invalid_phase_budget")
    require(plan["expected_timeout_phase"] is None or plan["expected_timeout_phase"] in PHASES,
            "invalid_expected_timeout")
    require(isinstance(plan["departing_instances"], list)
            and all(isinstance(value, str) for value in plan["departing_instances"])
            and len(plan["departing_instances"]) == len(set(plan["departing_instances"])),
            "invalid_registered_departures")
    for value in plan["departing_instances"]:
        digest(value)
    return json.loads(json.dumps(plan))


def immutable_json(path, value):
    data = (json.dumps(value, sort_keys=True, indent=2) + "\n").encode()
    fd = os.open(path, os.O_WRONLY | os.O_CREAT | os.O_EXCL, 0o600)
    with os.fdopen(fd, "wb") as stream:
        stream.write(data)
        stream.flush()
        os.fsync(stream.fileno())
    return hashlib.sha256(data).hexdigest()


class Lifecycle:
    """One sequential cohort, retaining failures through reconciliation/cleanup.

    The caller owns actual deadlines, cancellation and resource custody. This
    ledger detects late returns but cannot stop an operation. It never disposes
    resources or starts another cohort. An incomplete caller leaves its durable
    start record in events.jsonl without an invented end or final receipt.
    """

    def __init__(self, directory, plan, monotonic=time.monotonic, utc=None):
        self.plan = validate_plan(plan)
        self.directory = Path(directory)
        self.directory.mkdir(mode=0o700, parents=False, exist_ok=False)
        self.plan_hash = immutable_json(self.directory / "plan.json", self.plan)
        self.stream = (self.directory / "events.jsonl").open("x", encoding="utf8")
        os.chmod(self.directory / "events.jsonl", 0o600)
        self.clock = monotonic
        self.utc = utc or (lambda: datetime.datetime.now(datetime.timezone.utc).isoformat())
        self.origin = self.clock()
        self.last = 0.0
        self.sequence = 0
        self.active = None
        self.completed = []
        self.finalized = False
        self.custody = None

    def _now(self):
        value = (self.clock() - self.origin) * 1000
        require(math.isfinite(value) and value >= self.last, "monotonic_clock_regressed")
        self.last = value
        return value

    def _event(self, phase, boundary, elapsed, status, receipt=None):
        row = {"schema": "eg_lifecycle_event_v1", "run_id": self.plan["run_id"],
               "cohort_id": self.plan["cohort_id"], "sequence": self.sequence,
               "monotonic_ms": elapsed, "utc": self.utc(), "phase": phase,
               "boundary": boundary, "status": status, "receipt_sha256": receipt}
        self.stream.write(json.dumps(row, separators=(",", ":")) + "\n")
        self.stream.flush()
        os.fsync(self.stream.fileno())
        self.sequence += 1

    def start(self, phase):
        require(not self.finalized and self.active is None, "phase_already_active_or_final")
        require(len(self.completed) < len(PHASES) and phase == PHASES[len(self.completed)],
                "phase_order_invalid")
        if phase == "local_destroy":
            require(self.custody is not None and self.custody[1], "custody_unconfirmed_before_disposal")
        elapsed = self._now()
        self._event(phase, "start", elapsed, "started")
        self.active = (phase, elapsed)

    def end(self, phase, status, receipt_sha256):
        require(not self.finalized, "already_finalized")
        require(self.active is not None and self.active[0] == phase, "phase_not_active")
        require(status in ("ok", "failed", "timed_out"), "invalid_phase_status")
        digest(receipt_sha256)
        elapsed = self._now()
        duration = elapsed - self.active[1]
        # Expected timeout is evidence only when the registered bound elapsed.
        if status == "timed_out":
            require(duration >= self.plan["budgets_ms"][phase], "premature_timeout_claim")
        elif duration > self.plan["budgets_ms"][phase]:
            status = "late_return"
        self._event(phase, "end", elapsed, status, receipt_sha256)
        self.completed.append({"phase": phase, "status": status, "duration_ms": duration,
                               "receipt_sha256": receipt_sha256})
        self.active = None

    def confirm_custody(self, custody):
        require(not self.finalized, "already_finalized")
        require(isinstance(custody, dict) and set(custody) == {
            "receipt_sha256", "registered_departures", "confirmed_process_exits",
            "collectors_closed", "fixture_closed", "no_late_effect_barrier", "resources_disposed",
        }, "invalid_custody_fields")
        digest(custody["receipt_sha256"])
        for field in ("registered_departures", "confirmed_process_exits"):
            require(isinstance(custody[field], list) and all(isinstance(v, str) for v in custody[field])
                    and len(custody[field]) == len(set(custody[field])),
                    "invalid_departure_set")
            for value in custody[field]:
                digest(value)
        for field in ("collectors_closed", "fixture_closed", "no_late_effect_barrier", "resources_disposed"):
            require(type(custody[field]) is bool, "invalid_custody_flag")
        exits = set(custody["registered_departures"]) == set(custody["confirmed_process_exits"])
        require(set(custody["registered_departures"]) == set(self.plan["departing_instances"]),
                "registered_departures_changed")
        safe = exits and all(custody[k] for k in ("collectors_closed", "fixture_closed", "no_late_effect_barrier"))
        # Persist the independent custody receipt before permitting local_destroy;
        # a crash after disposal must not lose the evidence that allowed it.
        self._event("child_wait", "custody", self._now(), "confirmed" if safe else "unconfirmed",
                    custody["receipt_sha256"])
        self.custody = (json.loads(json.dumps(custody)), safe)
        return safe

    def finalize(self, accounting, custody):
        require(not self.finalized, "already_finalized")
        require(isinstance(accounting, dict) and set(accounting) == set(COUNTS) | {"receipt_sha256"},
                "invalid_accounting_fields")
        digest(accounting["receipt_sha256"])
        for field in COUNTS:
            require(type(accounting[field]) is int and accounting[field] >= 0, "invalid_accounting_count")
        require(accounting["http_attempts"] >= accounting["submitted_events"] > 0,
                "invalid_attempt_event_counts")
        require(accounting["submitted_events"] == sum(accounting[k] for k in (
            "proven_unadmitted", "custody_established", "admission_uncertain")), "admission_partition_mismatch")
        require(accounting["custody_established"] == sum(accounting[k] for k in (
            "direct_terminal", "reconciled_terminal", "unresolved")), "custody_partition_mismatch")
        require(accounting["terminal_failures"] <= accounting["direct_terminal"] + accounting["reconciled_terminal"],
                "terminal_failure_count_invalid")
        safe = self.confirm_custody(custody)
        complete = self.active is None and len(self.completed) == len(PHASES)
        normal = complete and all(row["status"] == "ok" for row in self.completed)
        expected = self.plan["expected_timeout_phase"]
        stall = complete and expected is not None and all(
            row["status"] == ("timed_out" if row["phase"] == expected else "ok") for row in self.completed)
        accounted = all(accounting[k] == 0 for k in (
            "admission_uncertain", "unresolved", "terminal_failures", "duplicate_effects",
            "missing_effects", "stale_effects", "wrong_owner_effects")) and (
                accounting["confirmed_effects"] == accounting["expected_effects"])
        self.stream.flush()
        output_hash = hashlib.sha256((self.directory / "events.jsonl").read_bytes()).hexdigest()
        result = {"schema": "eg_lifecycle_result_v1", "run_id": self.plan["run_id"],
                  "cohort_id": self.plan["cohort_id"], "plan_sha256": self.plan_hash,
                  "events_sha256": output_hash, "phases": self.completed,
                  "complete": complete, "normal_boundaries_observed": normal,
                  "registered_stall_observed": stall, "accounting_closed": accounted,
                  "custody_confirmed": safe, "fixture_disposal_permitted": safe,
                  "resources_disposed": custody["resources_disposed"],
                  "disposal_without_confirmation": custody["resources_disposed"] and not safe,
                  "accounting": accounting, "custody": custody,
                  "evidence_status": "COMPLETE" if (normal if expected is None else stall) and safe and accounted
                  and custody["resources_disposed"] else "INCOMPLETE",
                  "application_acceptance": "REQUIRES_INDEPENDENT_REVIEW"}
        immutable_json(self.directory / "result.json", result)
        self.finalized = True
        self.stream.close()
        return result

    def close(self):
        # Closing the recorder never asserts completion or deletes its evidence.
        self.stream.close()
