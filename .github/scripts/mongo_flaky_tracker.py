"""Keep one issue listing tests that passed only on their merge-queue retry.

CI runs with contents: read and issues: read, so a merge-queue Mongo shard
cannot write an issue. Each shard that saw a test fail and then pass on its
owner-approved retry uploads a small JSON record instead (see
mongo_flaky_retry.py). This scheduled collector, holding issues: write and
actions: read but nothing else, reads those records and keeps exactly one
issue titled "Flaky tests retried in the merge queue": each test with its
count, shards and recent run links, and the retry's expiry date.

It is idempotent. The issue body carries the collected state, keyed by
artifact id, so a record is counted once however often this runs, and an
unchanged body is never rewritten. The issue is found by a hidden marker in
issues this workflow's token created, open or closed, so it is never
duplicated; a closed one reopens only when a new retried pass appears.
Records are accepted only from merge-group runs of ci.yml in this repository.
"""
import base64
import datetime
import io
import json
import os
import re
import subprocess
import sys
import zipfile
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))
import mongo_flaky_retry as retry  # noqa: E402 - sibling module, not an installed package.

ROOT = Path(__file__).resolve().parents[2]
WORKFLOW = ROOT / ".github/workflows/ci-rust.yml"
CI_PATH = ".github/workflows/ci.yml"
QUEUE_BRANCH = "gh-readonly-queue/develop/"
BOT = "github-actions[bot]"
MARKER = "<!-- cma-mongo-flaky-retry-tracker -->"
STATE = re.compile(r"<!-- cma-mongo-flaky-state:([A-Za-z0-9+/=]*) -->")
LABELS = ["go-live-should-fix", "priority:P2", "track:infra-ci-release"]
# Matches the shards' upload retention; after it no record can remain.
RETENTION_DAYS = 3
MAX_RECORD_BYTES = 64 * 1024
MAX_ARTIFACT_IDS = 2000
RECENT_RUNS = 5
MAX_TABLE_ROWS = 100
MAX_COMMENT_ROWS = 50
# Rust test paths and nextest binary ids (cma::bin/cma); nothing that could
# break out of the Markdown code span the issue renders them in.
TEXT = re.compile(r"[A-Za-z0-9_:/.-]{1,300}")


class GitHub:
    """Read and write this repository through gh, each call bounded in time."""

    def __init__(self, repository, run=subprocess.run, timeout=60):
        self.repository, self._run, self._timeout = repository, run, timeout

    def _call(self, arguments, body=None):
        result = self._run(["gh", "api", *arguments], input=body, capture_output=True, timeout=self._timeout)
        if result.returncode != 0:
            raise RuntimeError(f"gh api {arguments[-1].split('?')[0]} failed: "
                               f"{result.stderr.decode('utf-8', 'replace').strip()[-300:]}")
        return result.stdout

    def get(self, path):
        return json.loads(self._call([f"repos/{self.repository}/{path}"]))

    def get_bytes(self, path):
        return self._call([f"repos/{self.repository}/{path}"])

    def send(self, method, path, payload):
        return json.loads(self._call(["-X", method, f"repos/{self.repository}/{path}", "--input", "-"],
                                     json.dumps(payload).encode("utf-8")))


def read_until(text):
    """The single CMA_MONGO_FLAKY_RETRY_UNTIL date that ci-rust.yml sets."""
    found = re.findall(rf'^\s+{retry.UNTIL_VARIABLE}: "([^"]*)"\s*$', text, re.MULTILINE)
    if len(found) != 1 or retry.parse_until(found[0]) is None:
        raise ValueError(f"ci-rust.yml must set {retry.UNTIL_VARIABLE} exactly once to a YYYY-MM-DD date")
    return retry.parse_until(found[0])


def empty_state():
    return {"artifacts": [], "tests": {}}


def parse_state(body):
    match = STATE.search(body or "")
    if not match:
        return empty_state()
    state = json.loads(base64.b64decode(match.group(1), validate=True))
    if not (isinstance(state, dict) and isinstance(state.get("artifacts"), list)
            and isinstance(state.get("tests"), dict)):
        raise ValueError("Tracking issue state is malformed")
    return state


def find_issue(api):
    """The one tracking issue this workflow created, open or closed, or None."""
    found = []
    for page in range(1, 11):
        rows = api.get(f"issues?state=all&creator={BOT.replace('[', '%5B').replace(']', '%5D')}"
                       f"&sort=created&direction=asc&per_page=100&page={page}")
        found += [row for row in rows if "pull_request" not in row and row.get("title") == retry.ISSUE_TITLE
                  and MARKER in (row.get("body") or "")]
        if len(rows) < 100:
            break
    return min(found, key=lambda row: row["number"]) if found else None


def shard_names():
    return [(shard, attempt, retry.artifact_name(shard, attempt))
            for shard in retry.ORDINARY_SHARDS for attempt in range(1, retry.MAX_ATTEMPT + 1)]


def plain(value):
    return isinstance(value, str) and TEXT.fullmatch(value) is not None


def validate_run(artifact, run, repository):
    """Refuse, before any download, an artifact a merge-group ci.yml run of this repository did not upload."""
    origin = artifact.get("workflow_run") or {}
    if not (run.get("event") == retry.EVENT and run.get("path") == CI_PATH
            and (run.get("repository") or {}).get("full_name") == repository
            and (run.get("head_repository") or {}).get("full_name") == repository
            and str(run.get("head_branch", "")).startswith(QUEUE_BRANCH)
            and origin.get("repository_id") == origin.get("head_repository_id")
            and type(run.get("id")) is int and run["id"] == origin.get("id")):
        raise ValueError("artifact did not come from a merge-group CI run of this repository")


def validate(record, run, repository, shard, attempt):
    """Refuse a record that does not describe exactly its run, attempt and shard."""
    if not isinstance(record, dict):
        raise ValueError("record is not a JSON object")
    tests = record.get("tests")
    if not (record.get("schema") == retry.RECORD_SCHEMA and record.get("repository") == repository
            and record.get("run_id") == run["id"]
            and record.get("run_attempt") == attempt and record.get("event") == retry.EVENT
            and record.get("shard") == shard and isinstance(tests, list)
            and 0 < len(tests) <= retry.MAX_RECORD_TESTS):
        raise ValueError("record does not match its artifact")
    for test in tests:
        if not (isinstance(test, dict) and plain(test.get("binary")) and plain(test.get("test"))
                and type(test.get("failed_attempts")) is int and test["failed_attempts"] >= 1):
            raise ValueError("record names a test outside the plain test-name alphabet")


def read_record(api, artifact):
    if not (type(artifact.get("size_in_bytes")) is int and 0 < artifact["size_in_bytes"] <= MAX_RECORD_BYTES):
        raise ValueError("artifact is not a small flaky record")
    archive = zipfile.ZipFile(io.BytesIO(api.get_bytes(f"actions/artifacts/{artifact['id']}/zip")))
    entries = archive.infolist()
    if [entry.filename for entry in entries] != [retry.RECORD_FILE] or entries[0].file_size > MAX_RECORD_BYTES:
        raise ValueError("artifact must hold exactly the flaky record")
    return json.loads(archive.read(retry.RECORD_FILE))


def collect(api, state, out):
    """New records, oldest first, skipping every artifact the state already counted."""
    known, found = set(state["artifacts"]), []
    for shard, attempt, name in shard_names():
        listing = api.get(f"actions/artifacts?name={name}&per_page=100")
        for artifact in listing.get("artifacts", []):
            if artifact.get("name") != name or artifact.get("expired") is not False or artifact.get("id") in known:
                continue
            try:
                run = api.get(f"actions/runs/{int(artifact['workflow_run']['id'])}")
                validate_run(artifact, run, api.repository)
                record = read_record(api, artifact)
                validate(record, run, api.repository, shard, attempt)
                found.append({"artifact": artifact["id"], "created": str(artifact.get("created_at", "")),
                              "url": f"{run['html_url']}/attempts/{attempt}", "run_id": run["id"],
                              "attempt": attempt, "shard": shard, "tests": record["tests"]})
            except (KeyError, TypeError, ValueError, zipfile.BadZipFile, RuntimeError) as error:
                # Unrecorded, so the next hourly run reads it again.
                print(f"::warning title=Flaky record skipped::{retry.command_data(name)} artifact "
                      f"{artifact.get('id')}: {retry.command_data(error)}", file=out, flush=True)
    return sorted(found, key=lambda row: (row["created"], row["artifact"]))


def merge(state, records):
    """Count each new record once; returns the occurrences it added."""
    added = []
    known = set(state["artifacts"])
    for record in records:
        if record["artifact"] in known:
            continue
        known.add(record["artifact"])
        state["artifacts"].append(record["artifact"])
        for test in record["tests"]:
            key = f"{test['binary']} {test['test']}"
            row = state["tests"].setdefault(key, {"binary": test["binary"], "test": test["test"], "count": 0,
                                                  "shards": [], "first_seen": record["created"], "runs": []})
            row["count"] += 1
            row["last_seen"] = record["created"]
            row["shards"] = sorted(set(row["shards"]) | {record["shard"]}, key=retry.ORDINARY_SHARDS.index)
            row["runs"] = ([{"url": record["url"], "run_id": record["run_id"], "attempt": record["attempt"]}]
                           + row["runs"])[:RECENT_RUNS]
            added.append((key, record))
    state["artifacts"] = sorted(state["artifacts"])[-MAX_ARTIFACT_IDS:]
    return added


def when(stamp):
    return stamp.replace("T", " ")[:16] if stamp else "?"


def render(state, until, today):
    window = (f"active through **{until.isoformat()}** (UTC, inclusive)" if today <= until else
              f"**ended on {until.isoformat()}** (UTC); the shards run without retries again")
    rows = sorted(state["tests"].values(), key=lambda row: (-row["count"], row["test"], row["binary"]))
    lines = [
        MARKER,
        "Agent Manager Mongo contract tests that **failed and then passed on their single automatic retry** in "
        "the develop merge queue. Each one is still a defect: fix the test or the race it exposes instead of "
        "relying on the retry.",
        "",
        "- **Scope (owner-approved 2026-11-24):** merge-group runs of the ordinary Mongo shards 1/8 to 8/8 only. "
        "Pull requests, pushes and the isolated shard never retry.",
        f"- **Retry window:** {window}.",
        f"- **Renewal:** a reviewed PR that changes `{retry.UNTIL_VARIABLE}` in `.github/workflows/ci-rust.yml`.",
        "",
        "| Test | Passed on retry | Shards | First seen (UTC) | Last seen (UTC) | Recent runs |",
        "|---|---:|---|---|---|---|",
    ]
    for row in rows[:MAX_TABLE_ROWS]:
        runs = ", ".join(f"[{run['run_id']}#{run['attempt']}]({run['url']})" for run in row["runs"])
        lines.append(f"| `{row['test']}` (`{row['binary']}`) | {row['count']} | {', '.join(row['shards'])} | "
                     f"{when(row['first_seen'])} | {when(row.get('last_seen'))} | {runs} |")
    if len(rows) > MAX_TABLE_ROWS:
        lines.append(f"\n{len(rows) - MAX_TABLE_ROWS} more test(s) are counted but not shown.")
    encoded = base64.b64encode(json.dumps(state, sort_keys=True, separators=(",", ":")).encode()).decode()
    lines += ["", "Collected hourly by `.github/workflows/mongo-flaky-tracker.yml` from the shards' "
              f"`{retry.ARTIFACT_PREFIX}-*` artifacts, which are kept {RETENTION_DAYS} days.", "",
              f"<!-- cma-mongo-flaky-state:{encoded} -->", ""]
    return "\n".join(lines)


def comment(added):
    lines = ["New tests passed only on their merge-queue retry:", ""]
    for key, record in added[:MAX_COMMENT_ROWS]:
        lines.append(f"- `{key}`: Mongo shard {record['shard']}, [run {record['run_id']} attempt "
                     f"{record['attempt']}]({record['url']})")
    if len(added) > MAX_COMMENT_ROWS:
        lines.append(f"- and {len(added) - MAX_COMMENT_ROWS} more")
    return "\n".join(lines) + "\n"


def track(api, until, today, out=sys.stdout):
    if today > until + datetime.timedelta(days=RETENTION_DAYS + 1):
        print(f"The merge-queue retry ended on {until.isoformat()} and every record has expired; "
              "nothing to collect.", file=out, flush=True)
        return 0
    issue = find_issue(api)
    state = parse_state(issue["body"]) if issue else empty_state()
    added = merge(state, collect(api, state, out))
    if issue is None and not added:
        print("No test has passed only on its merge-queue retry.", file=out, flush=True)
        return 0
    body = render(state, until, today)
    if issue is None:
        created = api.send("POST", "issues", {"title": retry.ISSUE_TITLE, "body": body, "labels": LABELS})
        print(f"Opened tracking issue #{created['number']} with {len(added)} retried pass(es).", file=out, flush=True)
        return 0
    if issue["state"] == "closed" and not added:
        print(f"Tracking issue #{issue['number']} is closed and nothing new was retried.", file=out, flush=True)
        return 0
    change = {"body": body} if body != issue.get("body") else {}
    if issue["state"] == "closed":
        change["state"] = "open"
    if change:
        api.send("PATCH", f"issues/{issue['number']}", change)
    if added:
        api.send("POST", f"issues/{issue['number']}/comments", {"body": comment(added)})
    print(f"Tracking issue #{issue['number']}: {len(added)} new retried pass(es)"
          f"{', reopened' if change.get('state') else ''}.", file=out, flush=True)
    return 0


def main():
    until = read_until(WORKFLOW.read_text(encoding="utf-8"))
    return track(GitHub(os.environ["GITHUB_REPOSITORY"]), until, retry.utc_today())


if __name__ == "__main__":
    raise SystemExit(main())
