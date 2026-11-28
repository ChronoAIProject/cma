"""Compare archived selections to the pre-split scripts without rebuilding.

Run from api/ with CMA_NEXTEST_REUSE=1. Arguments: baseline scripts directory,
output directory. The baseline is exported from the reviewed parent commit.
Old cargo-test selections are enumerated by the archived libtest executables
with the original libtest arguments, independently of nextest's filter DSL.
"""

from collections import Counter
import json
import os
from pathlib import Path
import re
import runpy
import shlex
import subprocess
import sys

ROOT = Path(__file__).resolve().parents[2]
RUNNER = ROOT / ".github/scripts/backend-nextest.sh"


def commands(path, prefix):
    text = path.read_text().replace("\\\n", " ")
    return [shlex.split(line) for line in text.splitlines()
            if line.startswith(prefix)]


def list_tests(arguments):
    result = subprocess.run(
        ["bash", str(RUNNER), "list", "--message-format", "json", *arguments],
        check=True, text=True, stdout=subprocess.PIPE,
    )
    metadata = json.loads(result.stdout)
    selected = {
        (binary, name)
        for binary, suite in metadata["rust-suites"].items()
        for name, case in suite["testcases"].items()
        if case["filter-match"]["status"] == "matches"
    }
    return selected, metadata


def old_nextest(args):
    # Package selection during build becomes a package predicate for reused
    # binaries. Every old -E expression and ignored policy remains unchanged.
    expr = args[args.index("-E") + 1]
    if "-p" in args:
        expr = f"package(={args[args.index('-p') + 1]}) and ({expr})"
    ignored = args[args.index("--run-ignored") + 1] if "--run-ignored" in args else "default"
    return list_tests(["-E", expr, "--run-ignored", ignored])[0]


def old_libtest(args, suites):
    split = args.index("--")
    cargo, libtest = args[2:split], args[split + 1:]
    package = cargo[cargo.index("-p") + 1]
    targets = [cargo[i + 1] for i, arg in enumerate(cargo) if arg == "--test"]
    filters = []
    index = 0
    while index < len(cargo):
        arg = cargo[index]
        if arg in ("-p", "--test"):
            index += 2
            continue
        if not arg.startswith("--"):
            filters.append(arg)
        index += 1
    selected = set()
    for binary, suite in suites.items():
        if suite["package-name"] != package:
            continue
        if "--lib" in cargo and suite["kind"] != "lib":
            continue
        if targets and (suite["kind"] != "test" or suite["binary-name"] not in targets):
            continue
        result = subprocess.run(
            [suite["binary-path"], *filters, *libtest, "--list", "--format", "terse"],
            check=True, text=True, stdout=subprocess.PIPE, cwd=suite["cwd"],
        )
        selected.update((binary, row.removesuffix(": test"))
                        for row in result.stdout.splitlines() if row.endswith(": test"))
    return selected


def mongo_filters(source, registry_directory=None):
    """Read corpus modes and the optional CPU profile without running fixtures."""
    template = shlex.split(next(line.strip() for line in source.splitlines()
                                if line.strip().startswith("-E ")))[1]
    if "$suite_filter" in template:
        registry = runpy.run_path(str(ROOT / ".github/scripts/mongo-contract-registry.py"))
        directory = registry_directory or ROOT / ".github/scripts/mongo-contract-suites"
        template = template.replace("$suite_filter", registry["suite_filter"](directory))
    names = dict(shlex.split(line)[1].split("=", 1) for line in source.splitlines()
                 if re.match(r"readonly [a-z0-9_]+_test=", line))
    default = re.search(r"local selection_filter=([\"'])(.*?)\1", source)
    predicates = {"all": default.group(2) if default else "all()"}
    optional = ("owner-allocation", "audit-bounds", "diagnostics", "scan-cooperation", "bson-ceiling", "first-response", "history", "cpu-profile",
                "observation-budget", "ranked-projection", "tc070-control")
    for mode in ("scale", *optional, "ordinary"):
        line = next((line.strip() for line in source.splitlines()
                     if line.strip().startswith(f"{mode})")), None)
        if mode in optional and line is None:
            continue
        predicate = re.search(r'''selection_filter=(["'])(.*?)\1''', line).group(2)
        # The pre-split source selected cma during compilation; reuse must
        # express the same package restriction in the filter instead.
        if "packages=(-p cma)" in line:
            predicate = "package(=cma) and " + predicate
        predicates[mode] = predicate
    # The listing default may name on-demand qualifications by variable too.
    for variable, name in names.items():
        predicates = {mode: predicate.replace(f"${variable}", name)
                      for mode, predicate in predicates.items()}
    filters = {mode: template.replace("$selection_filter", predicate)
               for mode, predicate in predicates.items()}
    if any("$" in expression for expression in filters.values()):
        raise ValueError("Unresolved Mongo filter variable")
    return filters


def main():
    if len(sys.argv) != 3 or os.environ.get("CMA_NEXTEST_REUSE") != "1":
        raise SystemExit("usage: CMA_NEXTEST_REUSE=1 python3 backend-selection-proof.py BASELINE_SCRIPTS OUTPUT")
    baseline, output = map(Path, sys.argv[1:])
    output.mkdir(parents=True, exist_ok=True)
    _, metadata = list_tests(["--run-ignored", "all"])
    suites = metadata["rust-suites"]
    reports = []

    def compare(label, old, new):
        if not old or old != new:
            raise AssertionError(f"{label}: old={len(old)}, new={len(new)}, missing={old-new}, extra={new-old}")
        reports.append({"selection": label, "old": len(old), "new": len(new)})
        (output / f"{label}.json").write_text(json.dumps(sorted(old), indent=2) + "\n")
        print(f"{label}: old={len(old)} new={len(new)} equal", flush=True)

    expr = "not (binary(=media_image_peak) or binary(=turn_metrics))"
    old = list_tests(["-E", expr])[0]
    shards = [list_tests(["-E", expr, "--partition", f"count:{i}/3"])[0] for i in range(1, 4)]
    counts = Counter(test for shard in shards for test in shard)
    if any(count != 1 for count in counts.values()):
        raise AssertionError("main shards overlap")
    compare("main-shard-union", old, set(counts))
    print(f"shard sizes: {[len(shard) for shard in shards]}; duplicates=0", flush=True)
    for binary in ["media_image_peak", "turn_metrics"]:
        expected = {(bid, name) for bid, suite in suites.items() if suite["binary-name"] == binary
                    for name, test in suite["testcases"].items() if not test["ignored"]}
        compare(binary, expected, list_tests(["-E", f"binary(={binary})"])[0])
    layer4 = "supervisor::mutation_tests::workspace_reconciliation_tests::real_linux_writer_boundary_reconciles_target_and_resumes_live_sibling"
    args = ["cargo", "test", "--locked", "-p", "layer4-runtime", layer4,
            "--", "--ignored", "--exact", "--test-threads=1"]
    compare("layer4-exact", old_libtest(args, suites), list_tests([
        "-E", f"package(=layer4-runtime) and test(={layer4})", "--run-ignored", "only"])[0])
    before = commands(baseline / "image-manager-mongo-contract.sh", "cargo ")
    before = [args for args in before if args[1] != "build"
              and not any("cma-image-manager" in arg for arg in args)]
    after = commands(ROOT / ".github/scripts/authenticated-mongo-contract.sh", "contract_nextest ")
    if len(before) != len(after):
        raise AssertionError("Authenticated Mongo invocation count changed")
    for index, (old, new) in enumerate(zip(before, after), 1):
        expected = old_libtest(old, suites) if old[1] == "test" else old_nextest(old)
        compare(f"authenticated-mongo-{index:02}", expected, old_nextest(new))
    old_source = (baseline / "mongo-contract-tests.sh").read_text()
    new_source = (ROOT / ".github/scripts/mongo-contract-tests.sh").read_text()
    before_filters = mongo_filters(old_source, baseline / "mongo-contract-suites")
    after_filters = mongo_filters(new_source)
    isolated = ("owner-allocation", "audit-bounds", "diagnostics", "scan-cooperation", "bson-ceiling", "first-response", "history", "ranked-projection", "tc070-control")
    mongo_sets = {}
    for mode in isolated:
        mongo_sets[mode] = list_tests(["-E", after_filters[mode], "--run-ignored", "only"])[0]
        if len(mongo_sets[mode]) != 1:
            raise AssertionError(f"Exactly one Mongo {mode} contract is required")
    for mode in ("all", "scale", "ordinary"):
        old = list_tests(["-E", before_filters[mode], "--run-ignored", "only"])[0]
        new = list_tests(["-E", after_filters[mode], "--run-ignored", "only"])[0]
        if mode == "ordinary":
            for exclusive in isolated:
                if exclusive not in before_filters:
                    if not mongo_sets[exclusive] <= old:
                        raise AssertionError(f"Isolated {exclusive} was absent from old ordinary corpus")
                    old -= mongo_sets[exclusive]
        compare(f"mongo-{mode}", old, new)
        mongo_sets[mode] = new
    counts = Counter(test for mode in ("scale", *isolated, "ordinary")
                     for test in mongo_sets[mode])
    if len(mongo_sets["scale"]) != 1 or any(count != 1 for count in counts.values()):
        raise AssertionError("Mongo exclusive phases and ordinary partitions must not overlap")
    compare("mongo-split-union", mongo_sets["all"], set(counts))
    for mode in isolated:
        if mode in before_filters:
            compare(f"mongo-{mode}", list_tests([
                "-E", before_filters[mode], "--run-ignored", "only"])[0], mongo_sets[mode])
    if "cpu-profile" in after_filters:
        profiles = list_tests(["-E", after_filters["cpu-profile"], "--run-ignored", "only"])[0]
        expected = {(binary, name) for binary, name in mongo_sets["ordinary"]
                    if suites[binary]["package-name"] == "cma" and "::cpu_profile::" in name}
        compare("mongo-cpu-profile", expected, profiles)
        if "cpu-profile" in before_filters:
            compare("mongo-cpu-profile-preserved", list_tests([
                "-E", before_filters["cpu-profile"], "--run-ignored", "only"])[0], profiles)
    compare("cmaeg-latency", old_nextest(["-p", "cmaeg", "-E", "test(latency_budget)", "--run-ignored", "ignored-only"]),
            list_tests(["-E", "package(=cmaeg) and test(~latency_budget)", "--run-ignored", "only"])[0])
    (output / "summary.json").write_text(json.dumps({"selections": reports, "shards": [len(s) for s in shards], "duplicates": 0}, indent=2) + "\n")


if __name__ == "__main__":
    main()
