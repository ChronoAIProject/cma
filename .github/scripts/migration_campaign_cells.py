#!/usr/bin/env python3
"""Map executed campaign stages onto plan cells without certifying any (#1327).

`derive` reads one private kind campaign record and keeps only what is
already public in source: stage names, verdicts, collection and check names,
and hashes that bind the evidence and reader artifacts. `aggregate` gives
every plan cell exactly one status:

- EXECUTED: a passing stage read or wrote that collection with that reader
  role under a writer the stage determines (old-write and new-read-old-write
  exercise the `retained` fixtures, whose writer is the old reader).
- EXECUTED_POLICY_UNATTRIBUTED: new-writer stages ran for the collection, but
  the campaign does not select the `disabled` or `enabled` writer policy, so
  neither cell can be credited.
- NOT_RUN: no passing stage touched the collection.

No status is a matrix pass. Certification still needs every cell's named
checks (decode, replay, restart, takeover, ...) in migration-matrix.mjs
evidence, which these fixtures do not produce.
"""
import argparse
import hashlib
import json
from pathlib import Path
import re
import sys

from migration_plan import identity
from migration_release_probe import declared_reader_artifacts, STAGE_ROLES


STAGES = tuple(STAGE_ROLES)
# Pairwise plans alias historical/recovery to the old reader and
# current/candidate to the new one (migration_plan.build).
READERS = {"old-write": ("historical", "recovery"), "new-read-old-write": ("current", "candidate"),
           "new-write": ("current", "candidate"), "old-read-new-write": ("historical", "recovery")}
RETAINED = {"old-write", "new-read-old-write"}
NAME = re.compile(r"^[A-Za-z0-9_.-]{1,120}$")
DIGEST = re.compile(r"^[a-f0-9]{64}$")


def require(condition, message):
    if not condition:
        raise ValueError("campaign cells: " + message)


def names(values, what):
    require(isinstance(values, list) and all(isinstance(v, str) and NAME.fullmatch(v) for v in values),
            f"{what} must be plain names")
    return sorted(set(values))


def artifact_hash(value):
    require(isinstance(value, dict), "reader artifact missing")
    artifact = identity(value.get("source"), value.get("image"))
    return hashlib.sha256(json.dumps(artifact, sort_keys=True).encode()).hexdigest()


def release_binding(evidence, fixtures):
    probe = evidence["release_probe"]
    require(isinstance(probe, dict), "per-role execution evidence missing")
    policy = probe.get("writer_policy")
    artifacts = probe.get("artifacts")
    require(policy in ("disabled", "enabled") and isinstance(artifacts, dict)
            and set(artifacts) == set(STAGE_ROLES.values()), "per-role execution evidence missing")
    hashes = {role: artifact_hash(artifact) for role, artifact in artifacts.items()}
    controls = probe.get("stage_controls")
    require(isinstance(controls, list), "release stage controls missing")
    seen = set()
    for control in controls:
        require(isinstance(control, dict), "invalid release stage control")
        stage = control.get("stage")
        require(stage in STAGE_ROLES and stage not in seen, "invalid or repeated release stage control")
        role = STAGE_ROLES[stage]
        require(artifact_hash(control) == hashes[role], "release stage reader differs")
        require(control.get("writer_policy") == (policy if role == "candidate" else "disabled"),
                "release stage policy differs")
        seen.add(stage)
    for stage, fixture in fixtures.items():
        require(stage in seen and artifact_hash(fixture) == hashes[STAGE_ROLES[stage]],
                "release fixture reader or stage differs")
    if evidence.get("outcome") == "UNVERIFIED":
        require(list(fixtures) == list(STAGES) and seen == set(STAGES), "incomplete release campaign")
        require(all(fixture.get("outcome") == "PASS" and bool(fixture["checks"])
                    and all(passed is True for passed in fixture["checks"].values())
                    for fixture in fixtures.values()), "release fixture checks did not pass")
        require((evidence.get("cleanup") or {}).get("cluster_absent") is True, "release cleanup missing")
    return {"writer_policy": policy, "artifact_hashes": hashes}


def derive(evidence_path, fixture):
    raw = Path(evidence_path).read_bytes()
    evidence = json.loads(raw)
    require(isinstance(evidence, dict) and evidence.get("schema") == "cma-migration-kind-evidence-v1",
            "campaign schema differs")
    require(isinstance(fixture, str) and re.fullmatch(r"[a-z][a-z0-9-]{0,40}", fixture), "fixture name invalid")
    plan = evidence.get("plan_sha256")
    require(isinstance(plan, str) and DIGEST.fullmatch(plan), "plan binding missing")
    stages, fixtures = {}, {}
    for row in evidence.get("fixtures") or []:
        stage = row.get("stage")
        require(stage in STAGES and stage not in stages, "stage invalid or duplicated")
        checks = row.get("checks")
        require(isinstance(checks, dict), "stage checks missing")
        fixtures[stage] = row
        stages[stage] = {"outcome": row.get("outcome") if row.get("outcome") in ("PASS", "FAIL") else "INVALID",
                         "collections": names(row.get("collections"), "collections"),
                         "checks": names([key for key, value in checks.items() if value], "checks")}
    result = {"schema": "cma-migration-campaign-cells-v1", "fixture": fixture,
              "evidence_sha256": hashlib.sha256(raw).hexdigest(), "plan_sha256": plan,
              "outcome": str(evidence.get("outcome")), "stages": stages}
    if "release_probe" in evidence:
        result["release_probe"] = release_binding(evidence, fixtures)
    return result


def slug(name):
    return re.sub(r"[^a-z0-9]+", "-", name.lower()).strip("-")


def aggregate(plan_bytes, derived):
    plan = json.loads(plan_bytes)
    require(plan.get("schema") == "cma-migration-matrix-plan-v1", "plan schema differs")
    plan_sha = hashlib.sha256(plan_bytes).hexdigest()
    readers = declared_reader_artifacts(plan)
    hashes = {role: artifact_hash(artifact) for role, artifact in readers.items()}
    pairwise = readers["historical"] == readers["recovery"] and readers["current"] == readers["candidate"]
    fixtures = {row["id"]: row for row in plan["fixtures"]}
    observed = {}
    sources = []
    for record in derived:
        require(record.get("schema") == "cma-migration-campaign-cells-v1", "derived schema differs")
        require(record.get("plan_sha256") == plan_sha, f"{record.get('fixture')} ran a different plan")
        source = {key: record[key] for key in ("fixture", "evidence_sha256", "outcome")}
        sources.append(source)
        if record.get("outcome") != "UNVERIFIED":
            continue  # a blocked campaign credits nothing, even its passing stages
        release = record.get("release_probe")
        policy = None
        if release is not None:
            policy = release.get("writer_policy")
            require(policy in ("disabled", "enabled") and release.get("artifact_hashes") == hashes,
                    "release policy or artifact binding differs from plan")
            require(set(record["stages"]) == set(STAGES), "incomplete release campaign")
            source["writer_policy"] = policy
        else:
            require(pairwise, "pairwise coverage requires a pairwise plan")
        for stage, row in record["stages"].items():
            if row["outcome"] != "PASS":
                continue
            readers_at_stage = (STAGE_ROLES[stage],) if release is not None else READERS[stage]
            fixture_policy = "retained" if stage in RETAINED else policy
            label = f"{record['fixture']}:{policy}:{stage}" if policy else f"{record['fixture']}:{stage}"
            for collection in row["collections"]:
                for reader in readers_at_stage:
                    observed.setdefault((reader, slug(collection), fixture_policy), []).append(label)
    cells, counts = [], {"EXECUTED": 0, "EXECUTED_POLICY_UNATTRIBUTED": 0, "NOT_RUN": 0}
    for cell in plan["cells"]:
        fixture = fixtures[cell["fixture"]]
        key = (cell["reader"], cell["fixture"].removeprefix("collection-").rsplit("-", 1)[0])
        if (*key, fixture["policy"]) in observed:
            status, stages = "EXECUTED", observed[(*key, fixture["policy"])]
        elif fixture["policy"] != "retained" and (*key, None) in observed:
            status, stages = "EXECUTED_POLICY_UNATTRIBUTED", observed[(*key, None)]
        else:
            status, stages = "NOT_RUN", []
        counts[status] += 1
        cells.append({"id": cell["id"], "status": status, "stages": sorted(set(stages))})
    return {"schema": "cma-migration-campaign-coverage-v1", "plan_sha256": plan_sha,
            "certified_cells": 0, "total_cells": len(cells), "counts": counts,
            "campaigns": sorted(sources, key=lambda row: (row["fixture"], row.get("writer_policy", ""))),
            "cells": cells}


def markdown(report):
    counts = report["counts"]
    lines = ["### Migration matrix coverage", "",
             f"- Plan sha256: `{report['plan_sha256']}`",
             f"- Cells: {report['total_cells']}; executed {counts['EXECUTED']}, "
             f"executed with unattributed writer policy {counts['EXECUTED_POLICY_UNATTRIBUTED']}, "
             f"not run {counts['NOT_RUN']}",
             "- Certified cells: 0. Certification needs every cell's named checks in matrix evidence.", "",
             "| Campaign | Writer policy | Outcome | Evidence sha256 |", "|---|---|---|---|"]
    lines += [f"| {row['fixture']} | {row.get('writer_policy', 'unattributed')} | "
              f"`{row['outcome']}` | `{row['evidence_sha256']}` |" for row in report["campaigns"]]
    return "\n".join(lines) + "\n"


def main(argv=None):
    parser = argparse.ArgumentParser(description=__doc__)
    commands = parser.add_subparsers(dest="command", required=True)
    one = commands.add_parser("derive")
    one.add_argument("--evidence", type=Path, required=True)
    one.add_argument("--fixture", required=True)
    one.add_argument("--output", type=Path, required=True)
    many = commands.add_parser("aggregate")
    many.add_argument("--plan", type=Path, required=True)
    many.add_argument("--output", type=Path, required=True)
    many.add_argument("derived", type=Path, nargs="+")
    args = parser.parse_args(argv)
    if args.command == "derive":
        args.output.write_text(json.dumps(derive(args.evidence, args.fixture), indent=2) + "\n")
        return
    report = aggregate(args.plan.read_bytes(), [json.loads(path.read_text()) for path in args.derived])
    args.output.write_text(json.dumps(report, indent=2) + "\n")
    sys.stdout.write(markdown(report))


if __name__ == "__main__":
    main()
