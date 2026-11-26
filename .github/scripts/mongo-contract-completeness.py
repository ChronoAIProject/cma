"""Fail closed when the ordinary Mongo selector omits an ignored CMA contract.

Nextest retains filtered testcases in its JSON listing. Audit that inventory,
not the maintained prefix list: a new module must be selected or explicitly
assigned another fixture. Exceptions are exact names so they cannot hide new
contracts added alongside an existing external test or subprocess helper.
"""

from collections import Counter
import json
from pathlib import Path
import sys


def check_selection(metadata, exceptions):
    suites = [suite for suite in metadata["rust-suites"].values()
              if (suite["package-name"], suite["binary-name"], suite["kind"]) == ("cma", "cma", "lib")]
    if len(suites) != 1:
        raise ValueError("Expected one complete CMA library test inventory")
    cases = suites[0]["testcases"]
    ignored = set()
    selected = set()
    for name, case in cases.items():
        status = case["filter-match"]["status"]
        if type(case["ignored"]) is not bool or status not in ("matches", "mismatch"):
            raise ValueError(f"Invalid test metadata: {name}")
        if case["ignored"]:
            ignored.add(name)
        if status == "matches":
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


if __name__ == "__main__":
    if len(sys.argv) != 2:
        raise SystemExit("usage: mongo-contract-completeness.py nextest-list.json")
    try:
        with Path(sys.argv[1]).open() as source:
            check_metadata(json.load(source))
    except (OSError, ValueError, KeyError, TypeError) as error:
        raise SystemExit(f"Mongo selection completeness failed: {error}") from error
