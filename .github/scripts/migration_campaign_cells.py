#!/usr/bin/env python3
"""Map executed campaign stages onto plan cells without certifying any (#1327).

`derive` reads one private kind campaign record and keeps only what is
already public in source: stage names, verdicts, collection and check names,
and the record's own sha256. `aggregate` joins those derived records to the
pairwise plan and gives every plan cell exactly one status:

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

STAGES = ("old-write", "new-read-old-write", "new-write", "old-read-new-write")
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


def derive(evidence_path, fixture):
    raw = Path(evidence_path).read_bytes()
    evidence = json.loads(raw)
    require(isinstance(evidence, dict) and evidence.get("schema") == "cma-migration-kind-evidence-v1",
            "campaign schema differs")
    require(isinstance(fixture, str) and re.fullmatch(r"[a-z][a-z0-9-]{0,40}", fixture), "fixture name invalid")
    plan = evidence.get("plan_sha256")
    require(isinstance(plan, str) and DIGEST.fullmatch(plan), "plan binding missing")
    stages = {}
    for row in evidence.get("fixtures") or []:
        stage = row.get("stage")
        require(stage in STAGES and stage not in stages, "stage invalid or duplicated")
        checks = row.get("checks")
        require(isinstance(checks, dict), "stage checks missing")
        stages[stage] = {"outcome": row.get("outcome") if row.get("outcome") in ("PASS", "FAIL") else "INVALID",
                         "collections": names(row.get("collections"), "collections"),
                         "checks": names([key for key, value in checks.items() if value], "checks")}
    return {"schema": "cma-migration-campaign-cells-v1", "fixture": fixture,
            "evidence_sha256": hashlib.sha256(raw).hexdigest(), "plan_sha256": plan,
            "outcome": str(evidence.get("outcome")), "stages": stages}


def slug(name):
    return re.sub(r"[^a-z0-9]+", "-", name.lower()).strip("-")


def aggregate(plan_bytes, derived):
    plan = json.loads(plan_bytes)
    require(plan.get("schema") == "cma-migration-matrix-plan-v1", "plan schema differs")
    plan_sha = hashlib.sha256(plan_bytes).hexdigest()
    readers = {row["id"] for row in plan["readers"]}
    require(readers == {"historical", "current", "candidate", "recovery"}, "plan readers differ")
    fixtures = {row["id"]: row for row in plan["fixtures"]}
    retained, unattributed = {}, {}
    sources = []
    for record in derived:
        require(record.get("schema") == "cma-migration-campaign-cells-v1", "derived schema differs")
        require(record.get("plan_sha256") == plan_sha, f"{record.get('fixture')} ran a different plan")
        sources.append({key: record[key] for key in ("fixture", "evidence_sha256", "outcome")})
        if record.get("outcome") != "UNVERIFIED":
            continue  # a blocked campaign credits nothing, even its passing stages
        for stage, row in record["stages"].items():
            if row["outcome"] != "PASS":
                continue
            for collection in row["collections"]:
                for reader in READERS[stage]:
                    target = retained if stage in RETAINED else unattributed
                    target.setdefault((reader, slug(collection)), []).append(f"{record['fixture']}:{stage}")
    cells, counts = [], {"EXECUTED": 0, "EXECUTED_POLICY_UNATTRIBUTED": 0, "NOT_RUN": 0}
    for cell in plan["cells"]:
        fixture = fixtures[cell["fixture"]]
        key = (cell["reader"], cell["fixture"].removeprefix("collection-").rsplit("-", 1)[0])
        if fixture["policy"] == "retained" and key in retained:
            status, stages = "EXECUTED", retained[key]
        elif fixture["policy"] != "retained" and key in unattributed:
            status, stages = "EXECUTED_POLICY_UNATTRIBUTED", unattributed[key]
        else:
            status, stages = "NOT_RUN", []
        counts[status] += 1
        cells.append({"id": cell["id"], "status": status, "stages": sorted(set(stages))})
    return {"schema": "cma-migration-campaign-coverage-v1", "plan_sha256": plan_sha,
            "certified_cells": 0, "total_cells": len(cells), "counts": counts,
            "campaigns": sorted(sources, key=lambda row: row["fixture"]), "cells": cells}


def markdown(report):
    counts = report["counts"]
    lines = ["### Migration matrix coverage", "",
             f"- Plan sha256: `{report['plan_sha256']}`",
             f"- Cells: {report['total_cells']}; executed {counts['EXECUTED']}, "
             f"executed with unattributed writer policy {counts['EXECUTED_POLICY_UNATTRIBUTED']}, "
             f"not run {counts['NOT_RUN']}",
             "- Certified cells: 0. Certification needs every cell's named checks in matrix evidence.", "",
             "| Campaign | Outcome | Evidence sha256 |", "|---|---|---|"]
    lines += [f"| {row['fixture']} | `{row['outcome']}` | `{row['evidence_sha256']}` |" for row in report["campaigns"]]
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
