"""Fail closed when the ordinary Mongo selector omits an ignored CMA contract.

Nextest retains filtered testcases in its JSON listing. Audit that inventory,
not the maintained prefix list: a new module must be selected or explicitly
assigned another fixture. Exceptions are exact names so they cannot hide new
contracts added alongside an existing external test or subprocess helper.
"""

import argparse
from collections import Counter
import json
from pathlib import Path
import runpy
import sys


def case_states(suite):
    for name, case in suite["testcases"].items():
        status = case["filter-match"]["status"]
        if type(case["ignored"]) is not bool or status not in ("matches", "mismatch"):
            raise ValueError(f"Invalid test metadata: {name}")
        yield name, case["ignored"], status == "matches"


def check_selection(metadata, exceptions):
    suite = metadata["rust-suites"]["cma::bin/cma"]
    if (suite["package-name"], suite["binary-name"], suite["kind"]) != ("cma", "cma", "bin"):
        raise ValueError("Expected the complete CMA binary test inventory")
    ignored = set()
    selected = set()
    for name, is_ignored, matches in case_states(suite):
        if is_ignored:
            ignored.add(name)
        if matches:
            selected.add(name)
    if not ignored or not selected or selected - ignored:
        raise ValueError("Mongo selection must contain only ignored contracts")

    for name, exception in exceptions.items():
        if (exception.get("kind") not in ("fixture", "helper", "deferred")
                or not exception.get("reason", "").strip()):
            raise ValueError(f"Exception needs a fixture, helper or tracked deferral reason: {name}")
        if exception["kind"] == "deferred" and not exception.get("issue", "").startswith("https://github.com/ChronoAIProject/cma/issues/"):
            raise ValueError(f"Deferred contract needs its tracking issue: {name}")
    stale = set(exceptions) - ignored
    unsafe = set(exceptions) & selected
    missing = ignored - selected - set(exceptions)
    problems = []
    for label, names in (("Stale exceptions", stale), ("Selected non-ordinary contracts", unsafe),
                         ("Omitted eligible Mongo contracts", missing)):
        if names:
            problems.append(label + ":\n  " + "\n  ".join(sorted(names)))
    if problems:
        raise ValueError("\n".join(problems))

    counts = Counter(entry["kind"] for entry in exceptions.values())
    print(f"Mongo selection completeness: {len(ignored)} ignored CMA tests; "
          f"{len(selected)} selected; {counts['fixture']} separate-fixture tests; "
          f"{counts['helper']} child helpers; {counts['deferred']} tracked deferrals; "
          "0 unclassified omissions", flush=True)
    for name, entry in sorted(exceptions.items()):
        if entry["kind"] == "deferred":
            print(f"Deferred Mongo contract: {name} ({entry['issue']})", flush=True)


def check_metadata(metadata):
    with Path(__file__).with_name("mongo-contract-exceptions.json").open() as source:
        check_selection(metadata, json.load(source))


def unique_object(pairs):
    result = {}
    for key, value in pairs:
        if key in result:
            raise ValueError(f"Duplicate JSON key: {key}")
        result[key] = value
    return result


def read_json(path):
    with path.open() as source:
        return json.load(source, object_pairs_hook=unique_object)


def inventory(metadata, prefixes):
    executables = {}
    cases = {}
    for binary_id, suite in metadata["rust-suites"].items():
        if suite["status"] != "listed" or suite["binary-id"] != binary_id:
            raise ValueError(f"Expected a full Nextest listing for {binary_id}")
        executables[binary_id] = tuple(suite[field] for field in (
            "package-name", "binary-name", "kind", "build-platform",
        ))
        for name, ignored, selected in case_states(suite):
            matched_suites = tuple(prefix for prefix in prefixes if name.startswith(prefix)) if selected else ()
            if selected and (not ignored or not matched_suites):
                raise ValueError(f"Expected ordinary Mongo selector metadata for {binary_id} {name}")
            cases[binary_id, name] = (suite["testcases"][name]["kind"], ignored, selected, matched_suites)
    if not cases or metadata["test-count"] != len(cases) or not any(case[2] for case in cases.values()):
        raise ValueError("Expected a complete Nextest case count and a nonempty Mongo selection")
    return executables, cases


def case_identity(value):
    if (not isinstance(value, list) or len(value) != 2
            or not all(isinstance(part, str) and part for part in value)):
        raise ValueError("Expected a case identity [binary-id, test name]")
    return tuple(value)


def compare_inventory(base, candidate, base_prefixes, candidate_prefixes, changes):
    base_executables, before = inventory(base, base_prefixes)
    candidate_executables, after = inventory(candidate, candidate_prefixes)
    if (not isinstance(changes, dict) or set(changes) - {"additions", "moves"}
            or any(not isinstance(entries, list) for entries in changes.values())):
        raise ValueError("Expected an object with additions and moves arrays")

    additions = set()
    for value in changes.get("additions", []):
        name = case_identity(value)
        if name in additions or name in before or name not in after:
            raise ValueError(f"Duplicate or stale addition: {name}")
        additions.add(name)

    moves = {}
    targets = set()
    for entry in changes.get("moves", []):
        if not isinstance(entry, dict) or set(entry) != {"from", "to"}:
            raise ValueError("Expected an exact move with from and to case identities")
        source, target = case_identity(entry["from"]), case_identity(entry["to"])
        if (source in moves or target in targets or target in additions
                or source not in before or source in after or target in before or target not in after):
            raise ValueError(f"Duplicate, stale or conflicting move: {source} -> {target}")
        if base_executables[source[0]][0] == candidate_executables[target[0]][0]:
            raise ValueError(f"Expected a package move: {source} -> {target}")
        moves[source] = target
        targets.add(target)

    expected = {moves.get(name, name): state for name, state in before.items()}
    missing = expected.keys() - after.keys()
    unexpected = after.keys() - expected.keys() - additions
    changed_coverage = {
        name for name in expected.keys() & after.keys()
        if expected[name] != after[name]
    }
    missing_executables = base_executables.keys() - candidate_executables.keys() - {name[0] for name in moves}
    new_executables = candidate_executables.keys() - base_executables.keys() - {name[0] for name in targets | additions}
    changed_executables = {
        binary for binary in base_executables.keys() & candidate_executables.keys()
        if base_executables[binary] != candidate_executables[binary]
    }
    problems = []
    for label, names in (
        ("Missing executables", missing_executables),
        ("Unexpected executables", new_executables),
        ("Changed executables", changed_executables),
        ("Missing cases", {" ".join(name) for name in missing}),
        ("Unexpected cases", {" ".join(name) for name in unexpected}),
        ("Changed case coverage", {" ".join(name) for name in changed_coverage}),
    ):
        if names:
            problems.append(label + ":\n  " + "\n  ".join(sorted(names)))
    if problems:
        raise ValueError("\n".join(problems))
    print(f"Nextest inventories agree: {len(before)} base cases; {len(after)} candidate cases; "
          f"{len(additions)} additions; {len(moves)} package moves; "
          f"{sum(case[2] for case in before.values())} base and "
          f"{sum(case[2] for case in after.values())} candidate Mongo selections", flush=True)


def compare_cli(argv):
    parser = argparse.ArgumentParser(description="Compare full Nextest listings from the ordinary Mongo selector.")
    parser.add_argument("base", type=Path)
    parser.add_argument("candidate", type=Path)
    parser.add_argument("--changes", type=Path, help="JSON object with exact additions and package moves")
    registry_script = Path(__file__).with_name("mongo-contract-registry.py")
    default_suites = registry_script.with_name("mongo-contract-suites")
    parser.add_argument("--base-suites", type=Path, default=default_suites)
    parser.add_argument("--candidate-suites", type=Path, default=default_suites)
    args = parser.parse_args(argv)
    load_prefixes = runpy.run_path(str(registry_script))["load_prefixes"]
    compare_inventory(read_json(args.base), read_json(args.candidate),
                      load_prefixes(args.base_suites), load_prefixes(args.candidate_suites),
                      read_json(args.changes) if args.changes else {})


if __name__ == "__main__":
    try:
        if sys.argv[1:2] == ["--compare"]:
            compare_cli(sys.argv[2:])
        elif len(sys.argv) == 2:
            check_metadata(read_json(Path(sys.argv[1])))
        else:
            raise SystemExit("usage: mongo-contract-completeness.py nextest-list.json or --compare --help")
    except (OSError, ValueError, KeyError, TypeError, AttributeError) as error:
        raise SystemExit(f"Mongo selection completeness failed: {error}") from error
