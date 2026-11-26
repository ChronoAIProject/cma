"""Select immutable Actions cache IDs that can be retired on develop."""
from __future__ import annotations

import re
from collections import defaultdict
from dataclasses import dataclass
from datetime import date, datetime, timezone
from pathlib import Path
from runpy import run_path

from staging_cd_build_cargo_keys import CARGO_NAMESPACE, identity

DEVELOP = "refs/heads/develop"
# The repository cache eviction limit is 100 GiB. Keep 10 GiB below it so
# this sweep, not GitHub's least-recently-used eviction, decides removals.
BUDGET = 90 * 1024 ** 3
PULL = re.compile(r"refs/pull/([1-9][0-9]*)/merge")
HEX = r"[a-f0-9]{64}"
INDEX = re.compile(r"(index-cma-(?:am|eg|web|runtime|runtime-omarchy)-1-[0-9a-f]{8})#([0-9]+)")
CARGO = re.compile(rf"{CARGO_NAMESPACE}(am|eg)-({HEX})-{HEX}-[1-9][0-9]*-[1-9][0-9]*")
OMARCHY = re.compile(rf"omarchy-layer-archive-v1-runtime-omarchy-deps-{HEX}")
# Default-branch CodeQL builds these databases. Its automatic overlay reader
# is the PR analysis; neither release nor develop CI restores this namespace.
CODEQL_BASE = re.compile(r"(codeql-overlay-base-database-1-[a-f0-9]+-[a-z+_-]+-[0-9]+\.[0-9]+\.[0-9]+)-[a-f0-9]{40}-[0-9]+-[0-9]+")
TRIVY = re.compile(r"(cache-trivy)-([0-9]{4}-[0-9]{2}-[0-9]{2})")
RUST_CI = re.compile(r"(v0-rust-(?:build|lint)-Linux-x64-[0-9a-f]{8})-[0-9a-f]{8}")
AUXILIARY = (
    re.compile(rf"(cma-develop-selection-v2-{HEX})-[a-f0-9]{{40}}-[0-9]+-[0-9]+"),
    CODEQL_BASE,
    TRIVY,
    RUST_CI,
)


@dataclass(frozen=True)
class Entry:
    id: int
    key: str
    ref: str
    version: str
    size: int
    created: datetime
    accessed: datetime

    @classmethod
    def parse(cls, row):
        def timestamp(field):
            value = datetime.fromisoformat(row[field].replace("Z", "+00:00"))
            if value.utcoffset() is None:
                raise ValueError("cache timestamp missing offset")
            return value

        if (type(row["id"]) is not int or row["id"] <= 0
                or type(row["size_in_bytes"]) is not int or row["size_in_bytes"] < 0
                or any(not isinstance(row[k], str) or not row[k] for k in ("key", "ref", "version"))):
            raise ValueError("invalid cache inventory entry")
        return cls(row["id"], row["key"], row["ref"], row["version"], row["size_in_bytes"],
                   timestamp("created_at"), timestamp("last_accessed_at"))


@dataclass(frozen=True)
class Policies:
    cargo: dict
    omarchy: str

    @classmethod
    def from_source(cls, root):
        cargo = {service: identity(root, service).family for service in ("am", "eg")}
        helper = run_path(str(Path(__file__).with_name("omarchy-cache-key.py")))
        return cls(cargo, "omarchy-layer-archive-v1-" + helper["scope"](helper["manifest"](root)))


@dataclass(frozen=True)
class Removal:
    entry: Entry
    reason: str
    successor: Entry | None = None
    spare: Entry | None = None


def recovery_kind(action, today, policy):
    old, keep = action.entry, action.successor
    if (not keep or old.ref != DEVELOP or keep.ref != DEVELOP or old.id == keep.id
            or old.version != keep.version or not keep.size
            or (keep.created, keep.id) <= (old.created, old.id)):
        return None
    overlay, replacement = CODEQL_BASE.fullmatch(old.key), CODEQL_BASE.fullmatch(keep.key)
    if overlay and replacement and overlay[1] == replacement[1]:
        # Default-branch CodeQL produces bases. Only best-effort PR overlays
        # download them, and a missing base falls back to full analysis.
        return "pr-overlay-base"
    trivy, replacement = TRIVY.fullmatch(old.key), TRIVY.fullmatch(keep.key)
    if trivy and replacement:
        try:
            old_day, keep_day = (date.fromisoformat(match[2]) for match in (trivy, replacement))
        except ValueError:
            return None
        # Trivy restores from the daily key family and refreshes its database
        # during the scan. Keep the same-day entry and require a later day.
        if old_day < keep_day <= today:
            return "superseded-trivy-database"
    if OMARCHY.fullmatch(old.key) and old.key != policy.omarchy and keep.key == policy.omarchy:
        # Omarchy restores by exact recipe and builds from pinned inputs on
        # a cache miss. Retain the archive used by current develop.
        return "superseded-omarchy-archive"
    rust_ci, replacement = RUST_CI.fullmatch(old.key), RUST_CI.fullmatch(keep.key)
    spare = action.spare
    spare_key = RUST_CI.fullmatch(spare.key) if spare else None
    if (rust_ci and replacement and spare_key and rust_ci[1] == replacement[1] == spare_key[1]
            and spare.ref == DEVELOP and spare.version == old.version and spare.size
            and spare.id not in (old.id, keep.id)
            and (spare.created, spare.id) > (old.created, old.id)
            and old.created.astimezone(timezone.utc).date() < today and old.accessed < keep.accessed):
        # The pinned Rust cache action tries the exact key and then this
        # environment prefix. It cleans stale target files after a partial hit.
        return "superseded-rust-ci-cache"
    cargo, replacement = CARGO.fullmatch(old.key), CARGO.fullmatch(keep.key)
    if cargo and replacement and cargo[1] == replacement[1] and old.created.astimezone(timezone.utc).date() < today:
        return "prior-day-v4-seed"
    return None


def generations(entries, policy):
    """Only retire entries with a compatible, visible replacement on develop."""
    groups = defaultdict(list)
    active = {(service, e.version) for e in entries if e.ref == DEVELOP and e.size
              for service, family in policy.cargo.items() if e.key.startswith(family) and CARGO.fullmatch(e.key)}
    for item in entries:
        if item.ref != DEVELOP:
            continue
        index = INDEX.fullmatch(item.key)
        cargo = CARGO.fullmatch(item.key)
        if index:
            group, rank = index[1], (int(index[2]),)
        elif cargo:
            service = cargo[1]
            group = f"cargo-{service}"
            rank = (item.key.startswith(policy.cargo[service]),)
            if (service, item.version) not in active:
                # Even before today's first save, duplicates within a previous
                # compiler family have a known-compatible fallback.
                group = f"previous-{service}-{cargo[2]}"
                rank = ()
        elif OMARCHY.fullmatch(item.key):
            group, rank = "omarchy", (item.key == policy.omarchy,)
        else:
            match = next((m for pattern in AUXILIARY if (m := pattern.fullmatch(item.key))), None)
            if not match:
                # REST lists shared blobs without their index membership. Every blob
                # could be reachable from a retained index, including other scopes.
                continue
            group, rank = match[1], ()
        groups[(group, item.version)].append(((*rank, item.created, item.id), item))

    for (group, _), candidates in groups.items():
        ordered = sorted(candidates, key=lambda pair: pair[0], reverse=True)
        if RUST_CI.fullmatch(ordered[0][1].key):
            usable = [item for _, item in ordered if item.size]
            if len(usable) > 2:
                keep, spare, *older = usable
                for old in older:
                    yield Removal(old, "superseded", keep, spare)
            continue
        rank, keep = ordered[0]
        if (group.startswith("cargo-") or group == "omarchy") and not rank[0]:
            # The latest reserved-source build may still need the previous family.
            # A missing/failed current upload supplies no evidence of replacement.
            continue
        if keep.size == 0:
            continue
        for _, old in ordered[1:]:
            if (group.startswith(("index-", "cargo-"))
                    and (old.created, old.id) > (keep.created, keep.id)):
                # A backfill/rerun or an index-counter reset can complete out of
                # order. The newest upload still needs an observable successor.
                continue
            yield Removal(old, "superseded", keep)


def plan(entries, policy):
    proposed = [Removal(item, "pull-request") for item in entries if PULL.fullmatch(item.ref)
                and not item.key.startswith(("cma-cd-cargo-v2-", "cma-cd-cargo-v3-"))]
    proposed.extend(generations(entries, policy))
    return sorted(proposed, key=lambda action: (-action.entry.size, action.entry.id))


@dataclass(frozen=True)
class Readers:
    develop: bool
    pulls: frozenset
    unknown_pull: bool

    def holds(self, ref):
        return (self.develop if ref == DEVELOP else self.unknown_pull or ref in self.pulls)

    @classmethod
    def from_runs(cls, runs, own_id):
        develop = unknown = False
        pulls = set()
        for run in runs:
            if run["id"] == own_id or run["status"] == "completed":
                continue
            # PR restores are best effort. Only runs on the same merge ref
            # can restore its PR-local entries.
            if run["event"].startswith("pull_request"):
                related = run["pull_requests"]
                if not isinstance(related, list):
                    raise ValueError("invalid run PR associations")
                unknown |= not related
                for pr in related:
                    if type(pr["number"]) is not int or pr["number"] <= 0:
                        raise ValueError("invalid run PR number")
                    pulls.add(f"refs/pull/{pr['number']}/merge")
            elif run.get("head_branch") == "develop" or not run.get("head_branch"):
                develop = True
        return cls(develop, frozenset(pulls), unknown)
