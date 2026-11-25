"""Candidate-bound asset, CI, compatibility, and recovery observations."""
from datetime import datetime, timezone, timedelta
import json
from pathlib import Path
import subprocess
import tempfile

from staging_cd_common import matches, require

ENGINES = {"chromium", "firefox", "webkit"}
CHECK_KINDS = {"ci", "fixture", "browser"}


def fields(value, expected, message):
    require(isinstance(value, dict) and set(value) == set(expected), message)


def hash_value(value):
    return matches(r"[0-9a-f]{64}", value)


def timestamp(value):
    require(isinstance(value, str) and matches(r"\d{4}-\d\d-\d\dT\d\d:\d\d:\d\d(?:\.\d{1,6})?Z", value),
            "Evidence time is malformed")
    return datetime.fromisoformat(value[:-1]).replace(tzinfo=timezone.utc)


def asset_map(value):
    require(isinstance(value, dict) and bool(value) and len(value) <= 10000
            and all(matches(r"assets/[A-Za-z0-9_.-]+", name) and hash_value(digest)
                    for name, digest in value.items()), "Web asset inventory is malformed")
    return value


def check_web_assets(freeze, references, files):
    reviewed = freeze["web_assets"]
    fields(reviewed, {"previous_source", "previous_digest", "previous_manifest_sha256",
                      "candidate_manifest_sha256"}, "Frozen Web asset inventory is malformed")
    require(matches(r"[0-9a-f]{40}", reviewed["previous_source"])
            and matches(r"sha256:[0-9a-f]{64}", reviewed["previous_digest"]),
            "Previous Web identity is malformed")
    fields(references, {"previous", "current", "serving"}, "Web asset evidence is incomplete")
    previous, current, serving = (files.json(references[key]) for key in ("previous", "current", "serving"))
    require(references["previous"]["sha256"] == reviewed["previous_manifest_sha256"]
            and references["current"]["sha256"] == reviewed["candidate_manifest_sha256"],
            "Web asset manifest differs from freeze")
    for row in (previous, current):
        fields(row, {"source", "digest", "assets"}, "Web manifest is malformed")
        asset_map(row["assets"])
    expected = freeze["components"]["web"]
    require((previous["source"], previous["digest"]) == (reviewed["previous_source"], reviewed["previous_digest"]),
            "Previous Web artifact differs")
    require((current["source"], current["digest"]) == (expected["source"], expected["digest"]),
            "Current Web artifact differs")
    fields(serving, {"source", "digest", "assets"}, "Serving Web inventory is malformed")
    require((serving["source"], serving["digest"]) == (current["source"], current["digest"]),
            "Serving Web artifact differs")
    combined = asset_map(serving["assets"])
    for manifest in (previous, current):
        require(all(combined.get(name) == value for name, value in manifest["assets"].items()),
                "A retained or candidate Web asset is missing or changed")
    return len(previous["assets"])


def validate_checks(required, previous):
    require(isinstance(required, dict) and 0 < len(required) <= 256,
            "Required qualification inventory is missing")
    for name, row in required.items():
        require(matches(r"[a-z][a-z0-9_-]{0,95}", name), "Qualification name is malformed")
        fields(row, {"kind", "minimum_samples", "inheritance"}, "Required qualification is malformed")
        require(row["kind"] in CHECK_KINDS and type(row["minimum_samples"]) is int
                and row["minimum_samples"] > 0, "Qualification scope is malformed")
        inheritance = row["inheritance"]
        if inheritance is not None:
            fields(inheritance, {"candidate_sha256", "source", "impact_sha256"},
                   "Evidence inheritance decision is malformed")
            require(previous is not None and inheritance["candidate_sha256"] == previous
                    and matches(r"[0-9a-f]{40}", inheritance["source"])
                    and hash_value(inheritance["impact_sha256"]), "Evidence inheritance is unpinned")
    require({row["kind"] for row in required.values()} == CHECK_KINDS,
            "CI, fixture and browser qualification are all required")


def common_observation(row, candidate, environment, samples):
    require(row["candidate_sha256"] == candidate and row["environment"] == environment
            and row["status"] == "PASS" and row["attempt"] == 1 and type(row["attempt"]) is int
            and row["cancelled"] is False and type(row["samples"]) is int and row["samples"] >= samples
            and timestamp(row["finished_at"]) > timestamp(row["started_at"]),
            "Qualification is incomplete, retried or bound to another candidate")


def check_gate_observations(freeze, candidate_sha, observations, files, observed_at):
    required = freeze["checks"]
    fields(observations, required, "Required CI or fixture evidence is missing")
    browsers = {}
    run_ids = set()
    for name, rule in required.items():
        item = observations[name]
        fields(item, {"result", "impact"}, "Qualification reference is malformed")
        row = files.json(item["result"])
        fields(row, {"name", "kind", "candidate_sha256", "environment", "started_at", "finished_at",
                     "samples", "status", "attempt", "cancelled", "outcome"},
               "Qualification result is malformed")
        require(row["name"] == name and row["kind"] == rule["kind"], "Qualification result identity differs")
        inheritance = rule["inheritance"]
        if inheritance is None:
            require(item["impact"] is None, "Fresh evidence carries an inheritance decision")
            bound, source = candidate_sha, freeze["source"]
        else:
            impact = files.json(item["impact"])
            require(item["impact"]["sha256"] == inheritance["impact_sha256"]
                    and impact == {"name": name, "previous_candidate_sha256": inheritance["candidate_sha256"],
                                   "current_source": freeze["source"], "current_candidate_sha256": candidate_sha,
                                   "decision": "inherit",
                                   "scope": "unaffected"}, "Qualification lacks its reviewed impact decision")
            bound, source = inheritance["candidate_sha256"], inheritance["source"]
        common_observation(row, bound, freeze["environment"], rule["minimum_samples"])
        require(timestamp(row["finished_at"]) <= observed_at, "Qualification result postdates observation")
        outcome = row["outcome"]
        if rule["kind"] == "ci":
            fields(outcome, {"run_id", "head_sha", "conclusion"}, "CI result is malformed")
            require(type(outcome["run_id"]) is int and outcome["run_id"] > 0
                    and outcome["head_sha"] == source and outcome["conclusion"] == "success",
                    "Required CI did not pass for the bound source")
            run_ids.add(outcome["run_id"])
        elif rule["kind"] == "browser":
            fields(outcome, {"engine", "asset_404", "blank_pages", "duplicate_commands", "lost_drafts"},
                   "Browser result is malformed")
            engine = outcome["engine"]
            require(engine in ENGINES and all(type(outcome[key]) is int and outcome[key] == 0
                                               for key in set(outcome) - {"engine"}),
                    "Retained-tab browser qualification failed")
            browsers[engine] = browsers.get(engine, 0) + row["samples"]
        else:
            fields(outcome, {"passed", "failures"}, "Fixture outcome is malformed")
            require(outcome["passed"] is True and type(outcome["failures"]) is int and outcome["failures"] == 0,
                    "Required fixture did not pass")
    require(set(browsers) == ENGINES and all(samples >= 20 for samples in browsers.values()),
            "Retained-tab browser qualification requires twenty observations per engine")
    return {"count": len(required), "ci_runs": len(run_ids), "browser_samples": browsers}


def verify_matrix(freeze, refs, files, candidate, repair, observed_at):
    reviewed = freeze["matrix"]
    fields(reviewed, {"source_inventory_sha256", "shape_inventory_sha256", "plan_sha256", "bindings_sha256"},
           "Frozen reader matrix identity is malformed")
    fields(refs, {"source_inventory", "shape_inventory", "plan", "bindings", "evidence"},
           "Reader matrix is incomplete")
    for key, expected in reviewed.items():
        require(hash_value(expected) and refs[key.removesuffix("_sha256")]["sha256"] == expected,
                "Reviewed reader matrix inventory differs")
    # Run the existing matrix verifier against captured bytes including all raw cell receipts.
    # A different file with the same name cannot replace a validated input during the run.
    with tempfile.TemporaryDirectory(prefix="cma-reader-matrix-") as temp:
        root = Path(temp)
        paths = {}
        for key, ref in refs.items():
            path = root / (key + ".json")
            path.write_bytes(files.read(ref))
            paths[key] = path
        plan = json.loads(paths["plan"].read_bytes())
        require(plan["candidate"] == candidate, "Reader matrix is for a different candidate")
        recovery = [row["id"] for row in plan["readers"]
                    if row["role"] == "recovery" and row["artifact"] == repair]
        require(bool(recovery) and all(cell["expectation"] == "accept" for cell in plan["cells"]
                                       if cell["reader"] in recovery),
                "Executable repair cannot read every reviewed retained shape")
        matrix = json.loads(paths["evidence"].read_bytes())
        require(matrix["configuration_sha256"] == freeze["state"]["configuration"]
                and timestamp(matrix["finished_at"]) <= observed_at,
                "Reader matrix configuration or time window differs")
        evidence_parent = refs["evidence"]["path"].rpartition("/")[0]
        for cell in matrix["cells"]:
            ref = cell["raw"]
            source = dict(ref, path=(evidence_parent + "/" if evidence_parent else "") + ref["path"])
            data = files.read(source)
            target = root / ref["path"]
            target.parent.mkdir(parents=True, exist_ok=True)
            if target.exists():
                require(target.read_bytes() == data, "Reader matrix raw receipt path is ambiguous")
            else:
                target.write_bytes(data)
        script = Path(__file__).with_name("migration-matrix-inventory.mjs")
        result = subprocess.run(["node", str(script), str(paths["plan"]), str(paths["evidence"]),
                                 str(paths["source_inventory"]), reviewed["source_inventory_sha256"],
                                 str(paths["shape_inventory"]), reviewed["shape_inventory_sha256"],
                                 str(paths["bindings"]), candidate["source"], candidate["image"]],
                                capture_output=True, text=True, timeout=120, check=False)
        require(result.returncode == 0, "Reader matrix verification failed")
        return json.loads(result.stdout)


def check_recoveries(freeze, candidate_sha, references, files, repair, observed_at):
    require(isinstance(references, list) and len(references) >= 2 and len(references) <= 16,
            "Two distinct compatible recovery rehearsals are required")
    run_ids, raw_hashes, windows = set(), set(), set()
    initial = 0
    for reference in references:
        row = files.json(reference)
        fields(row, {"run_id", "candidate_sha256", "environment", "started_at", "finished_at", "samples",
                     "status", "attempt", "cancelled", "kind", "repair", "first_failure_at", "probes",
                     "retained_before", "retained_after", "database_rollback", "history_rewritten",
                     "home_deleted", "asset_404", "new_turn_available"}, "Recovery result is malformed")
        common_observation(row, candidate_sha, freeze["environment"], 1)
        require(hash_value(row["run_id"]) and row["run_id"] not in run_ids
                and reference["sha256"] not in raw_hashes, "Recovery rehearsals must be distinct")
        run_ids.add(row["run_id"])
        raw_hashes.add(reference["sha256"])
        require(row["kind"] in ("initial_qualification", "frozen_recheck")
                and row["repair"] == repair, "Recovery uses an unreviewed executable")
        start, failure, finish = map(timestamp, (row["started_at"], row["first_failure_at"], row["finished_at"]))
        require(start <= failure <= finish <= observed_at and finish - failure <= timedelta(minutes=30),
                "Compatible recovery exceeded thirty minutes or postdates observation")
        require((start, failure, finish) not in windows, "Recovery rehearsal window was duplicated")
        windows.add((start, failure, finish))
        if row["kind"] == "initial_qualification":
            require(finish <= timestamp(freeze["frozen_at"]), "Initial qualification must precede freeze")
            initial += 1
        else:
            require(start >= timestamp(freeze["frozen_at"]), "Frozen recheck predates freeze")
        probes = row["probes"]
        require(isinstance(probes, list) and len(probes) >= 2 and len(probes) <= 10000,
                "Stable recovery probe window is missing")
        observed = []
        for probe in probes:
            fields(probe, {"at", "ready"}, "Recovery probe is malformed")
            require(probe["ready"] is True, "Recovery probe window was interrupted")
            observed.append(timestamp(probe["at"]))
        require(failure <= observed[0] <= observed[-1] <= finish
                and observed[-1] - observed[0] >= timedelta(minutes=5)
                and all(timedelta(0) < later - earlier <= timedelta(seconds=30)
                        for earlier, later in zip(observed, observed[1:])),
                "Five continuous minutes of stable probes are unproven")
        fields(row["retained_before"], {"database", "history", "homes"}, "Retained recovery state is incomplete")
        require(all(hash_value(value) for value in row["retained_before"].values())
                and row["retained_after"] == row["retained_before"]
                and row["database_rollback"] is False and row["history_rewritten"] is False
                and row["home_deleted"] is False and type(row["asset_404"]) is int and row["asset_404"] == 0
                and row["new_turn_available"] is True, "Compatible recovery did not preserve retained state")
    require(initial >= 2, "Frozen rechecks cannot replace initial qualification rehearsals")
    return {"initial_qualifications": initial, "total": len(run_ids)}
