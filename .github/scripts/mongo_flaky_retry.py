"""One owner-approved automatic retry for flaky merge-queue Mongo shards.

The repository owner approved this gate change on 2026-11-24: in the develop
merge queue, the Agent Manager's ordinary Mongo shards (1/8 ... 8/8) retry a
failing test once. Pull request, push and every other event keep zero
retries, and so does the isolated shard: its dedicated measurement contracts
run under their own budgets, where a retry would hide a regression.

The retry ends after CMA_MONGO_FLAKY_RETRY_UNTIL, a UTC date (inclusive) set
once in .github/workflows/ci-rust.yml. After it, every shard runs without
retries again and prints one notice. Renewal is a reviewed PR that changes
that date; nothing else extends it.

A test that passes only on its retry is still a defect. `report` turns each
nextest JUnit <flakyFailure>/<flakyError> into a warning annotation and a job
summary row, and writes a small JSON record. CI holds no issue write grant, so
the scheduled .github/workflows/mongo-flaky-tracker.yml collects those
records into one tracking issue.

    policy  decide the retry count for this shard (workflow step outputs)
    report  flag tests that passed only on their retry
"""
import datetime
import json
import os
import re
import sys
import xml.etree.ElementTree as ElementTree
from pathlib import Path

ORDINARY_SHARDS = tuple(f"{index}/8" for index in range(1, 9))
TRACK = "am"
EVENT = "merge_group"
UNTIL_VARIABLE = "CMA_MONGO_FLAKY_RETRY_UNTIL"
ISSUE_TITLE = "Flaky tests retried in the merge queue"
ARTIFACT_PREFIX = "mongo-flaky-retry"
# The tracker looks up every name a shard can upload; a later re-run attempt
# than this still retries and annotates, but its record is not collected.
MAX_ATTEMPT = 3
RECORD_FILE = "flaky-tests.json"
RECORD_SCHEMA = "cma-mongo-flaky-retry/v1"
MAX_RECORD_TESTS = 200
MAX_MESSAGE = 300
# mongo-contract-tests.sh writes the per-partition nextest tool config (its
# JUnit path) under this directory, so it must also be a valid TOML string.
REPORT_DIRECTORY = re.compile(r"/[A-Za-z0-9._/-]+")
DATE = re.compile(r"[0-9]{4}-[0-9]{2}-[0-9]{2}")
FLAKY = ("flakyFailure", "flakyError")
FAILED = ("failure", "error")


def artifact_name(shard, attempt):
    """The upload name for one ordinary shard attempt: 3/8, attempt 1 -> ...-shard-3-of-8-attempt-1."""
    if shard not in ORDINARY_SHARDS or type(attempt) is not int or not 1 <= attempt:
        raise ValueError("Flaky records exist only for ordinary shard attempts")
    index, total = shard.split("/")
    return f"{ARTIFACT_PREFIX}-{TRACK}-shard-{index}-of-{total}-attempt-{attempt}"


def parse_until(value):
    """The inclusive last retry date, or None when the setting is not one YYYY-MM-DD date."""
    if not isinstance(value, str) or not DATE.fullmatch(value):
        return None
    try:
        return datetime.date.fromisoformat(value)
    except ValueError:
        return None


def utc_today():
    return datetime.datetime.now(datetime.timezone.utc).date()


def decide(event, track, shard, until, today):
    """(retries, annotation) for one shard; anything outside the approved scope gets zero.

    The annotation is a complete workflow command line, or None when the run is
    simply outside the scope (PR, push, isolated shard) and needs no notice.
    """
    if event != EVENT or track != TRACK or shard not in ORDINARY_SHARDS:
        return 0, None
    last = parse_until(until)
    if last is None:
        return 0, (f"::warning title=Merge-queue flaky retry disabled::{UNTIL_VARIABLE} is not a YYYY-MM-DD "
                   f"date, so Mongo shard {shard} runs without retries.")
    if today > last:
        return 0, (f"::notice title=Merge-queue flaky retry expired::The owner-approved retry ended on "
                   f"{last.isoformat()} (UTC, inclusive); Mongo shard {shard} runs without retries. Renewal is a "
                   f"reviewed PR that changes {UNTIL_VARIABLE} in .github/workflows/ci-rust.yml.")
    return 1, None


def write_outputs(path, values):
    with open(path, "a", encoding="utf-8") as output:
        for key, value in values.items():
            output.write(f"{key}={value}\n")


def policy(env, today=None, out=sys.stdout):
    shard = env.get("MONGO_SHARD", "")
    retries, annotation = decide(env.get("EVENT_NAME"), env.get("TRACK"), shard,
                                 env.get(UNTIL_VARIABLE), utc_today() if today is None else today)
    outputs = {"retries": str(retries)}
    if annotation:
        print(annotation, file=out, flush=True)
    if retries:
        attempt = int(env.get("GITHUB_RUN_ATTEMPT", ""))
        directory = Path(env.get("RUNNER_TEMP", "")) / ARTIFACT_PREFIX
        if not REPORT_DIRECTORY.fullmatch(str(directory)):
            raise SystemExit("Flaky retry report directory must be an absolute plain path")
        directory.mkdir(parents=True, exist_ok=True)
        outputs.update({"report-dir": str(directory), "artifact": artifact_name(shard, attempt)})
        print(f"Merge-queue flaky retry is active through {env[UNTIL_VARIABLE]} (UTC): a failing test in "
              f"Mongo shard {shard} runs once more, and a pass on that retry is still flagged.", file=out, flush=True)
    else:
        print(f"Mongo shard {shard or '?'} runs without retries.", file=out, flush=True)
    write_outputs(env["GITHUB_OUTPUT"], outputs)
    return 0


def command_data(text):
    """Escape workflow command data so one annotation stays one line."""
    return str(text).replace("%", "%25").replace("\r", "%0D").replace("\n", "%0A")


def first_line(text):
    line = next((row.strip() for row in str(text or "").splitlines() if row.strip()), "")
    return line[:MAX_MESSAGE]


def flaky_tests(junit):
    """Tests in one nextest JUnit report that failed and then passed on a retry.

    nextest records every failed attempt of a test that finally passed as
    <flakyFailure> or <flakyError>; a test that failed every attempt carries
    <failure> or <error> (plus <rerunFailure>) and is an ordinary failure.
    """
    root = ElementTree.parse(junit).getroot()
    if root.tag != "testsuites":
        raise ValueError("Not a nextest JUnit report")
    found = []
    for suite in root.iter("testsuite"):
        for case in suite.iter("testcase"):
            attempts = [child for child in case if child.tag in FLAKY]
            if not attempts or any(child.tag in FAILED for child in case):
                continue
            name = case.get("name")
            binary = case.get("classname") or suite.get("name")
            if not name or not binary:
                raise ValueError("A flaky JUnit test case has no name")
            found.append({"binary": binary, "test": name, "failed_attempts": len(attempts),
                          "message": first_line(attempts[0].get("message") or attempts[0].text)})
    return found


def markdown_cell(text):
    return str(text).replace("|", "\\|").replace("\n", " ").replace("`", "'")


def report(env, out=sys.stdout):
    directory = Path(env["CMA_MONGO_FLAKY_REPORT_DIR"])
    shard = env["MONGO_SHARD"]
    outcome = env.get("TESTS_OUTCOME", "")
    configs = sorted(directory.glob("nextest-*.toml"))
    missing = [config.name for config in configs
               if not (directory / ("junit-" + config.name[len("nextest-"):-len(".toml")] + ".xml")).is_file()]
    flaky = []
    for junit in sorted(directory.glob("junit-*.xml")):
        try:
            flaky.extend(flaky_tests(junit))
        except (ElementTree.ParseError, OSError, ValueError) as error:
            print(f"::error title=Flaky retry report unreadable::{command_data(junit.name)}: {command_data(error)}. "
                  "Tests that passed only on their retry cannot be confirmed absent.", file=out, flush=True)
            return 1
    # The runner writes a partition's tool config in the same command that adds
    # --retries, so a config without its JUnit report is a retried partition
    # whose flaky passes would go unreported.
    if outcome == "success" and missing:
        print(f"::error title=Flaky retry report missing::Mongo shard {shard} ran {len(missing)} retried "
              "partition(s) without a JUnit report, so a test that passed only on its retry cannot be "
              "confirmed absent.", file=out, flush=True)
        return 1
    if not configs:
        # No partition retried, so nothing can have passed on a retry.
        print(f"::warning title=Flaky retry not applied::The policy enabled one retry for Mongo shard {shard}, "
              "but no partition ran with it.", file=out, flush=True)
    if missing:
        print(f"{len(missing)} partition(s) wrote no JUnit report; the failed contracts step already fails "
              "this job.", file=out, flush=True)
    for test in flaky:
        print(f"::warning title=Flaky test (retried once)::{command_data(test['binary'])} "
              f"{command_data(test['test'])} failed {test['failed_attempts']} attempt(s) and passed only on its "
              f"retry in Mongo shard {shard} (merge queue). It is still a defect, tracked in \"{ISSUE_TITLE}\".",
              file=out, flush=True)
    lines = [f"### Merge-queue flaky retry: Mongo shard {shard}", ""]
    if flaky:
        lines += [f"{len(flaky)} test(s) failed and then passed on their single automatic retry. Each is "
                  f"still a defect; the scheduled tracker adds it to \"{ISSUE_TITLE}\".", "",
                  "| Test | Binary | Failed attempts | First failure |", "|---|---|---|---|"]
        lines += [f"| `{markdown_cell(t['test'])}` | `{markdown_cell(t['binary'])}` | {t['failed_attempts']} | "
                  f"{markdown_cell(t['message'])} |" for t in flaky]
    else:
        lines.append("No test needed its retry.")
    lines += ["", f"Retry active through {env.get(UNTIL_VARIABLE, '?')} (UTC, inclusive).", ""]
    with open(env["GITHUB_STEP_SUMMARY"], "a", encoding="utf-8") as summary:
        summary.write("\n".join(lines))
    if flaky:
        record = {
            "schema": RECORD_SCHEMA, "repository": env["GITHUB_REPOSITORY"],
            "run_id": int(env["GITHUB_RUN_ID"]), "run_attempt": int(env["GITHUB_RUN_ATTEMPT"]),
            "event": env.get("EVENT_NAME"), "ref": env.get("GITHUB_REF"), "sha": env.get("GITHUB_SHA"),
            "shard": shard, "retry_until": env.get(UNTIL_VARIABLE),
            "tests": [{key: test[key] for key in ("binary", "test", "failed_attempts")}
                      for test in flaky[:MAX_RECORD_TESTS]],
        }
        (directory / RECORD_FILE).write_text(json.dumps(record, indent=1, sort_keys=True) + "\n", encoding="utf-8")
    write_outputs(env["GITHUB_OUTPUT"], {"count": str(len(flaky)), "upload": "true" if flaky else "false"})
    return 0


def main(argv):
    if argv == ["policy"]:
        return policy(os.environ)
    if argv == ["report"]:
        return report(os.environ)
    print("usage: mongo_flaky_retry.py policy|report", file=sys.stderr)
    return 2


if __name__ == "__main__":
    raise SystemExit(main(sys.argv[1:]))
