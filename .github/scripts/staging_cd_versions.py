"""Plan immutable per-deployable versions from the merged PR and annotated tags."""
from __future__ import annotations

from dataclasses import dataclass

from deployables import components
from staging_cd_common import matches, require

DEPLOYABLES = components()
# These lanes finish a started build and queue only the newest reservation. A
# queued older reservation yields before building; once a build starts, only a
# higher final version fences it, never a successor's pending reservation.
FINISH_STARTED_BUILDS = frozenset({"am"})
LEVELS = {"version-upgrade:" + level: level for level in ("major", "minor", "patch")}
BASE_VERSION = "0.10.0"
REPOSITORY = "ChronoAIProject/cma"
SEMVER = r"(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)"


@dataclass(frozen=True)
class VersionPlan:
    component: str
    version: str
    tag: str
    repository: str
    receipt: dict | None = None


def version_tuple(version):
    require(matches(SEMVER, version) and len(version) <= 128, "Version must be a canonical stable semver")
    return tuple(int(part) for part in version.split("."))


def bump(version, level):
    major, minor, patch = version_tuple(version)
    require(level in LEVELS.values(), "Version upgrade level is invalid")
    if level == "major":
        return f"{major + 1}.0.0"
    if level == "minor":
        return f"{major}.{minor + 1}.0"
    return f"{major}.{minor}.{patch + 1}"


def exact_base(pr):
    base = pr.get("base")
    return isinstance(base, dict) and base.get("ref") == "develop" \
        and isinstance(base.get("repo"), dict) and base["repo"].get("full_name") == REPOSITORY


def merged_pr(revision, request):
    require(matches(r"[0-9a-f]{40}", revision), "Merged source revision is malformed")
    candidates = []
    for page in range(1, 11):
        rows = request(f"commits/{revision}/pulls?per_page=100&page={page}")
        require(isinstance(rows, list) and len(rows) <= 100, "Merged PR response is malformed")
        for row in rows:
            require(isinstance(row, dict), "Merged PR response is malformed")
            if row.get("merge_commit_sha") == revision and row.get("merged_at") is not None \
                    and exact_base(row):
                candidates.append(row.get("number"))
        if len(rows) < 100:
            break
    else:
        require(False, "Merged PR enumeration is incomplete")
    require(len(candidates) == 1 and type(candidates[0]) is int and candidates[0] > 0,
            "Source must belong to exactly one merged develop PR")
    pr = request(f"pulls/{candidates[0]}")
    require(isinstance(pr, dict) and pr.get("merged") is True and pr.get("merge_commit_sha") == revision
            and exact_base(pr) and pr.get("number") == candidates[0],
            "Merged PR identity changed")
    return pr


def label_level(pr):
    labels = pr.get("labels")
    require(isinstance(labels, list) and all(isinstance(label, dict) and isinstance(label.get("name"), str)
                                          for label in labels), "Version labels are malformed")
    upgrades = [label["name"] for label in labels if label["name"].startswith("version-upgrade:")]
    require(len(upgrades) == 1 and upgrades[0] in LEVELS,
            "Merged PR must carry exactly one supported version-upgrade label")
    return LEVELS[upgrades[0]]


def merged_pr_level(revision, request):
    pr = merged_pr(revision, request)
    return pr["number"], label_level(pr)


def plan_version(component, level, tags, source_revision):
    require(component in DEPLOYABLES and matches(r"[0-9a-f]{40}", source_revision),
            "Deployable or source revision is invalid")
    require(isinstance(tags, list), "Version tag inventory is malformed")
    deployable = DEPLOYABLES[component]
    prefix = deployable + "/v"
    highest, seen, published = BASE_VERSION, set(), []
    for tag in tags:
        require(isinstance(tag, dict) and isinstance(tag.get("name"), str), "Version tag inventory is malformed")
        if not tag["name"].startswith(prefix):
            continue
        name = tag["name"]
        require(name not in seen and tag.get("type") == "tag" and tag.get("annotation_name") == name
                and tag.get("target_type") == "commit" and matches(r"[0-9a-f]{40}", tag.get("target")),
                "Deployable version tag must be a unique annotated commit tag")
        seen.add(name)
        current = name[len(prefix):]
        if tag["target"] == source_revision:
            # The receipt validator is shared with registry recovery. Lazy
            # import keeps semver/identity rules independent of persistence.
            from staging_cd_receipts import validate_receipt
            receipt = validate_receipt(tag.get("receipt"))
            require(receipt["tag"] == name and receipt["source"] == source_revision,
                    "Existing version receipt differs from its tag")
            published.append(VersionPlan(component, current, name, receipt["repository"], receipt))
        if version_tuple(current) > version_tuple(highest):
            highest = current
    require(len(published) <= 1, "One source has conflicting deployable versions")
    if published:
        return published[0]
    version = bump(highest, level)
    return VersionPlan(component, version, prefix + version, "ghcr.io/chronoaiproject/" + deployable)


def plan_selected(deployables, revision, request, tags):
    require(isinstance(deployables, list) and all(isinstance(name, str) for name in deployables)
            and len(set(deployables)) == len(deployables)
            and set(deployables).issubset(DEPLOYABLES.values()), "Selected deployable list is invalid")
    require(matches(r"[0-9a-f]{40}", revision), "Merged source revision is malformed")
    if not deployables:
        return None, []
    selected = [tag for tag in tags if tag.get("target") == revision
                and tag.get("receipt", {}).get("identity", {}).get("component") in DEPLOYABLES
                and DEPLOYABLES[tag["receipt"]["identity"]["component"]] in deployables]
    pr = merged_pr(revision, request)
    pr_number = pr["number"]
    if selected:
        from staging_cd_receipts import validate_receipt
        frozen = [validate_receipt(tag["receipt"])["identity"] for tag in selected]
        require(all(item["pr"] == pr_number and item["bump"] == frozen[0]["bump"] for item in frozen),
                "Published components disagree about their merged PR")
        level = frozen[0]["bump"]
    else:
        level = label_level(pr)
    plans = [plan_version(component, level, tags, revision) for component, name in DEPLOYABLES.items()
             if name in deployables]
    return pr_number, plans
