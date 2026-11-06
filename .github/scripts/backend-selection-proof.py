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
    before = [args for args in before if args[1] != "build"]
    after = commands(ROOT / ".github/scripts/image-manager-mongo-contract.sh", "nextest_build ")
    if len(before) != len(after):
        raise AssertionError("Image Manager invocation count changed")
    for index, (old, new) in enumerate(zip(before, after), 1):
        expected = old_libtest(old, suites) if old[1] == "test" else old_nextest(old)
        compare(f"image-manager-{index:02}", expected, old_nextest(new))
    old_source = (baseline / "mongo-contract-tests.sh").read_text()
    new_source = (ROOT / ".github/scripts/mongo-contract-tests.sh").read_text()
    def mongo_selection(text):
        return shlex.split(next(line.strip() for line in text.splitlines() if line.strip().startswith("-E ")))[1]
    compare("mongo", list_tests(["-E", mongo_selection(old_source), "--run-ignored", "only"])[0],
            list_tests(["-E", mongo_selection(new_source), "--run-ignored", "only"])[0])
    compare("cmaeg-latency", old_nextest(["-p", "cmaeg", "-E", "test(latency_budget)", "--run-ignored", "ignored-only"]),
            list_tests(["-E", "package(=cmaeg) and test(~latency_budget)", "--run-ignored", "only"])[0])
    (output / "summary.json").write_text(json.dumps({"selections": reports, "shards": [len(s) for s in shards], "duplicates": 0}, indent=2) + "\n")


if __name__ == "__main__":
    main()
