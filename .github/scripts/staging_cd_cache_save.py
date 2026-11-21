"""Prove that a build's layer export created its Actions cache index, or say why not.

A develop build exports its layers with ignore-error, so a refused export leaves
publication green: once the Actions cache turns read-only (organization budget
reached), every later build of that component runs cold with no visible cause.
actions/cache/save annotates its own refusal; this export does not. After
publication the job reads the repository's cache list for the index the build
should have created and prints one fixed outcome line, plus one warning naming
the consequence when the index is missing.

This is diagnostic only. It never fails its step and prints only fixed text.
An unreadable cache list is "unknown", never "not saved".
"""
import datetime
import hashlib
import math
import re
import sys
import time

from ci_outcome import COMPONENTS, escape_data, escape_property
from staging_cd_common import require
from staging_cd_github import GitHub
from staging_cd_versions import REPOSITORY

REF = "refs/heads/develop"
# BuildKit (cache/remotecache/gha) keys a scope's layer index as
# "index-<scope>-<index version>-<first 8 hex of sha256(token write scope)>",
# and go-actions-cache appends "#<n>" for each new revision. A develop push
# writes with its own ref as the scope.
WRITER = hashlib.sha256(REF.encode()).hexdigest()[:8]
# The cache list is eventually consistent: absence counts only on the read made
# after every wait. Each read, with its own repeats, ends within READ_SECONDS.
WAITS = (5, 10, 15)
READ_SECONDS = 20
# A runner clock slightly ahead of the service must not hide a fresh index.
# Release jobs of one component never run concurrently, so an index this close
# before the build started cannot be an earlier staging export.
CLOCK_SKEW = 60
KEY = re.compile(r"[A-Za-z0-9][A-Za-z0-9._-]{0,511}")
# The build summary's fixed export classification and the reason it reports.
EXPORT_ERRORS = {"read-only": "read-only", "failed": "export-failed"}
REASONS = {"saved": None, "not-saved": "absent", "unknown": "unreadable"}
LAYER_WARNINGS = {
    "read-only": "{c} build layers were not saved: Actions cache is read-only: organization budget reached. "
                 "Later {c} builds will start cold and run slower until the budget allows cache writes again.",
    "export-failed": "{c} build layers were not saved: BuildKit reported that their cache export failed, and the "
                     "repository's Actions cache may be read-only (budget reached). "
                     "Later {c} builds will start cold and run slower.",
    "absent": "{c} build layers were not saved; the repository's Actions cache may be read-only (budget reached) "
              "or the export failed. Later {c} builds will start cold and run slower.",
}


def develop_push(env):
    return (env.get("GITHUB_REPOSITORY") == REPOSITORY and env.get("GITHUB_EVENT_NAME") == "push"
            and env.get("GITHUB_REF") == REF)


def created(text):
    value = datetime.datetime.fromisoformat(text.replace("Z", "+00:00"))
    require(value.tzinfo is not None, "Actions cache creation time is invalid")
    return value.timestamp()


def listing(api, prefix):
    """Return develop's cache entries whose key starts with prefix, as (key, creation time)."""
    require(isinstance(prefix, str) and KEY.fullmatch(prefix) is not None, "Actions cache key prefix is invalid")
    value = api.request("actions/caches?key=" + prefix + "&ref=" + REF + "&sort=created_at&direction=desc&per_page=100",
                        deadline=time.monotonic() + READ_SECONDS)
    require(isinstance(value, dict) and isinstance(value.get("actions_caches"), list), "Actions cache list is invalid")
    rows = []
    for row in value["actions_caches"]:
        require(isinstance(row, dict) and isinstance(row.get("key"), str) and isinstance(row.get("created_at"), str),
                "Actions cache entry is invalid")
        when = created(row["created_at"])
        # Entries of another ref were never written by this develop push.
        if row.get("ref") == REF:
            rows.append({"key": row["key"], "created": when})
    return rows


def observe(read, found, *, waits=WAITS, sleep=time.sleep):
    """Return "saved", "not-saved" or "unknown" after bounded, spaced reads.

    Any read showing the entry proves the save. Absence is concluded only from
    the last read, made after every wait; if that read fails the result is unknown.
    """
    result = "unknown"
    for wait in (0, *waits):
        if wait:
            sleep(wait)
        try:
            rows = read()
        except Exception:  # noqa: BLE001 - an unreadable list proves nothing either way.
            result = "unknown"
            continue
        if any(found(row) for row in rows):
            return "saved"
        result = "not-saved"
    return result


def emit(stream, cache, result, reason, *, component=None, title=None, warning=None):
    line = "CACHE-SAVE cache=" + cache + ("" if component is None else " component=" + component)
    line += " result=" + result + ("" if reason is None else " reason=" + reason)
    print(line, file=stream, flush=True)
    if warning is not None:
        print(f"::warning title={escape_property(title)}::{escape_data(warning)}", file=stream, flush=True)


def layer_outcome(scope, summary, started, read, **options):
    """(result, reason) for the index of a build that asked BuildKit to export its layers."""
    error = summary.get("cache_export_error") if isinstance(summary, dict) else None
    if error in EXPORT_ERRORS:
        # BuildKit writes the index after every layer, so an export error means
        # no new index was saved; there is nothing to wait for.
        return "not-saved", EXPORT_ERRORS[error]
    require(isinstance(started, (int, float)) and math.isfinite(started), "Build start time is invalid")
    index = re.compile(re.escape("index-" + scope + "-") + r"[0-9]+-" + WRITER + r"#[1-9][0-9]*")
    # CI image builds also export the am, eg and web scopes on develop pushes; any
    # fresh index is what the next build imports, so it counts as saved here.
    result = observe(lambda: read("index-" + scope + "-"),
                     lambda row: index.fullmatch(row["key"]) is not None and row["created"] >= started - CLOCK_SKEW,
                     **options)
    return result, REASONS[result]


def report_layers(component, scope, summary, started, env, *, stream=None, api=None, **options):
    """Print whether a develop build's layer export reached the Actions cache; never raise."""
    stream = sys.stdout if stream is None else stream
    if component not in COMPONENTS or not develop_push(env):
        return  # Only a develop push exports layers, so nothing else expects a save.
    try:
        require(isinstance(scope, str) and KEY.fullmatch(scope) is not None, "Build cache scope is invalid")
        reader = api if api is not None else GitHub(env.get("GH_TOKEN"))
        result, reason = layer_outcome(scope, summary, started, lambda prefix: listing(reader, prefix), **options)
    except Exception:  # noqa: BLE001 - a diagnostic never changes the publication outcome.
        result, reason = "unknown", REASONS["unknown"]
    warning = LAYER_WARNINGS[reason].format(c=component) if result == "not-saved" else None
    emit(stream, "build-layers", result, reason, component=component, title="Build cache not saved", warning=warning)
