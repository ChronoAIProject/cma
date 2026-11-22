"""Validate complete opt-in inventory evidence without trusting process success."""
import json

PREFIX = "store::sandbox_mongo_integration_tests::resource_access_tests::"
PERSONAL = PREFIX + "personal_inventory::"
LIST = PERSONAL + "concurrency::personal_agent_list_one_hundred_concurrent_public_readers_meet_budgets"
PROVENANCE = PERSONAL + "concurrency::personal_agent_provenance_one_hundred_concurrent_public_readers_meet_budgets"
LIST_PROFILE = PERSONAL + "concurrency::personal_agent_list_phase_profile"
PROVENANCE_PROFILE = PERSONAL + "concurrency::personal_agent_provenance_phase_profile"
ORGANIZATION = PERSONAL + "organization::organization_one_hundred_concurrent_public_readers_meet_budgets"
AVAILABILITY = PERSONAL + "availability::personal_inventory_constrained_availability_comparison"
OBSERVATION = PREFIX + "history_measurement::observation_budget::observation_one_hundred_observers_on_two_replicas_for_thirty_minutes"
MODES = {"agent-lists": [LIST, PROVENANCE, ORGANIZATION],
         "agent-list-profile": [LIST_PROFILE, PROVENANCE_PROFILE],
         "organization-inventory": [ORGANIZATION], "availability": [AVAILABILITY],
         "observation-budget": [OBSERVATION]}
MARKER = "CMA_INVENTORY_QUALIFICATION="


def require(value, message):
    if not value:
        raise ValueError(message)


def unique(pairs):
    require(len({key for key, _ in pairs}) == len(pairs), "duplicate JSON keys")
    return dict(pairs)


def parse(text):
    records = []
    for line in text.splitlines():
        if MARKER not in line:
            continue
        require(line.startswith(MARKER), "truncated or prefixed scenario record")
        records.append(json.loads(line[len(MARKER):], object_pairs_hook=unique))
    return records


def expected(mode):
    require(mode in MODES, "unknown qualification mode")
    result = {}
    for case in MODES[mode]:
        if case == OBSERVATION:
            for scenario, history, many, distinct in [("one-agent", 1000, False, False),
                    ("many-agents", 100, True, False), ("distinct-owners", 100, True, True)]:
                result[(case, scenario)] = {"kind": "observation", "history": history,
                    "many": many, "distinct": distinct, "instrumentation": "command-observer",
                    "expected_observers": 100, "replicas": 2, "required_seconds": 1800,
                    "opening_concurrency": 16}
        elif case == AVAILABILITY:
            for route in ["agents", "provenance"]:
                for variant in ["legacy", "batch"]:
                    result[(case, variant + "/" + route)] = {"kind": "availability", "route": route,
                        "mode": variant, "distinct": True, "instrumentation": "command-observer",
                        "expected_readers": 100, "expected_warmup": 100, "deadline_us": 60_000_000}
        else:
            organization = case == ORGANIZATION
            route = "organization" if organization else "provenance" if case in [PROVENANCE, PROVENANCE_PROFILE] else "agents"
            instrumentation = "mongo-profiler" if organization else "phase-and-mongo-profiler" if case in [LIST_PROFILE, PROVENANCE_PROFILE] else "command-observer"
            for scenario, distinct in [("shared", False), ("distinct", True)]:
                result[(case, scenario)] = {"kind": "readers", "route": route, "mode": "batch",
                    "distinct": distinct, "instrumentation": instrumentation,
                    "expected_readers": 100, "expected_warmup": 101 if organization else 100}
    return result


def count(value):
    return type(value) is int and value >= 0


def validate(mode, records):
    wanted = expected(mode)
    seen, summaries = set(), []
    for row in records:
        require(isinstance(row, dict), "invalid scenario record")
        key = (row.get("case"), row.get("scenario"))
        require(key in wanted and key not in seen, "unexpected or duplicate scenario")
        seen.add(key)
        details = wanted[key]
        require(row.get("schema") == "cma-inventory-qualification-v1" and
                json.dumps(row.get("details"), sort_keys=True) == json.dumps(details, sort_keys=True),
                "scenario schema, mode, route or instrumentation differs")
        require(row.get("completed") is True and row.get("database_cleanup") is True,
                "incomplete scenario or failed database cleanup")
        require(row.get("debug_assertions") is False, "unoptimized test configuration")
        require(row.get("runtime_workers") == (1 if key[0] == OBSERVATION else 4), "runtime worker configuration differs")
        require(count(row.get("started_ms")) and count(row.get("finished_ms")) and
                row["finished_ms"] >= row["started_ms"] and count(row.get("elapsed_us")), "invalid scenario timing")
        samples = row.get("samples")
        require(isinstance(samples, list) and len(samples) == 100, "incomplete reader cohort")
        require(all(isinstance(s, dict) and count(s.get("reader")) for s in samples) and
                sorted(s["reader"] for s in samples) == list(range(100)), "duplicate or missing reader")
        require(all(s.get("status") == 200 and s.get("semantic_match") is True and
                s.get("deadline") is False and count(s.get("elapsed_us")) for s in samples),
                "request failure, deadline or changed public result")
        times = sorted(s["elapsed_us"] for s in samples)
        commands = row.get("commands")
        require(isinstance(commands, dict) and commands and all(count(v) for v in commands.values()), "missing command counts")
        total = sum(commands.values())
        if key[0] == OBSERVATION:
            duration = row.get("stable_elapsed_us")
            require(count(duration) and duration >= 1_800_000_000, "incomplete thirty-minute window")
            require(row.get("active_observers") == 100 and row.get("head_agents") == row.get("agents") and
                    row.get("agents") == (100 if details["many"] else 1), "observer cohort changed")
            require(count(row.get("stable_started_ms")) and count(row.get("stable_finished_ms")) and
                    row["stable_finished_ms"] - row["stable_started_ms"] >= 1_800_000, "invalid stable timestamps")
            require(total * 1_000_000 <= duration * 100, "idle command budget exceeded")
            require(count(row.get("snapshots")) and 0 < row["snapshots"] <= row["agents"] * 1801,
                    "snapshot budget exceeded")
            gap = row.get("minimum_refresh_gap_us")
            require(count(row.get("max_head_refreshes")) and row["max_head_refreshes"] <= 1801 and
                    ((gap is None and row["max_head_refreshes"] <= 1) or (count(gap) and gap >= 1_000_000)),
                    "head refresh budget exceeded")
            require(all("agent_log" not in name for name in commands), "idle history scan")
            opening = row.get("opening_commands")
            require(isinstance(opening, dict) and opening and all(count(v) for v in opening.values()), "missing opening counts")
        else:
            require(row.get("warmup_completed") == details["expected_warmup"], "incomplete warmup/reference cohort")
            require(row.get("failed_commands") == 0, "failed Mongo command")
            if key[0] == AVAILABILITY:
                require(row.get("status_counts") == {"200": 100} and row.get("diagnostic_complete") is True,
                        "availability statuses differ")
                require(max(times) < 60_000_000, "availability deadline exceeded")
            else:
                organization = key[0] == ORGANIZATION
                require(times[94] < 1_000_000 if organization else times[94] <= 500_000, "p95 budget exceeded")
                require(times[98] < 2_000_000, "p99 budget exceeded")
                require(total == (1200 if organization else 900 if details["route"] == "provenance" else 800), "command budget differs")
        summaries.append({"case": key[0], "scenario": key[1], "readers": 100,
            "instrumentation": details["instrumentation"], "p95_us": times[94], "p99_us": times[98]})
    require(seen == set(wanted), "missing scenarios; partial diagnostics cannot qualify")
    return summaries
