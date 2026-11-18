"""Explicit source retirement never substitutes for a published replacement."""
import hashlib
import json
import subprocess
from pathlib import Path, PurePosixPath

from staging_cd_common import matches, require
from staging_cd_receipts import canonical
from staging_cd_reservations import pending_name, validate_reservation
from staging_cd_versions import DEPLOYABLES, exact_base, merged_pr

MANIFEST = Path(".github/staging-retirements.json")
SOURCE = "730c1bfcb804e86531e838f84bd2133013a6f4a2"
TREE = "6b6d525856a587a8eb3b2c59349ce7b92fbbc2d8"
RETIRED_COMPONENTS = frozenset({"runtime", "runtime-desktop"})
BURN_OBJECTS = {"runtime": "c090d4f98c3f92c39f2891bb9814ffb57f1566cc",
                "runtime-desktop": "5a54c932adea660758424b4435df6d0c0c08aa86"}
INPUT_ROOT = "api/crates/layer4-runtime/"


def original_reservation(component):
    return {"schema": "cma-version-reservation-v1", "component": component,
            "version": "0.10.1", "previous": "0.10.0", "source": SOURCE, "tree": TREE,
            "pr": 2331, "bump": "patch", "run_id": 44199012676, "run_attempt": 2}


def unique(pairs):
    result = {}
    for key, value in pairs:
        require(key not in result, "Duplicate retirement policy field")
        result[key] = value
    return result


def parse_policy(data):
    require(isinstance(data, bytes) and len(data) <= 128 * 1024, "Retirement policy exceeds its bound")
    policy = json.loads(data, object_pairs_hook=unique)
    require(isinstance(policy, dict) and set(policy) == {"schema", "decisions"}
            and policy["schema"] == "cma-source-retirements-v1" and isinstance(policy["decisions"], list),
            "Retirement policy schema is invalid")
    seen, identifiers = set(), set()
    for entry in policy["decisions"]:
        require(isinstance(entry, dict) and set(entry) == {
            "id", "reason", "reservation", "reservation_tag", "reservation_object", "replacement_pr", "packaging_inputs"},
            "Retirement decision schema is invalid")
        reservation = validate_reservation(entry["reservation"])
        component = reservation["component"]
        require(component in RETIRED_COMPONENTS and component not in seen
                and reservation == original_reservation(component)
                and entry["reservation_tag"] == pending_name(reservation)
                and entry["reservation_object"] == BURN_OBJECTS[component]
                and entry["id"] == "retire-" + component + "-730c1bfc"
                and entry["id"] not in identifiers and entry["reason"] == "private-key-test-material",
                "Unknown, changed or duplicate retirement decision")
        require(type(entry["replacement_pr"]) is int and entry["replacement_pr"] > 2331,
                "Retirement replacement PR must be concrete")
        inputs = entry["packaging_inputs"]
        require(isinstance(inputs, dict) and 1 <= len(inputs) <= 30
                and INPUT_ROOT + "Dockerfile" in inputs, "Retirement packaging inputs are missing")
        for name, digest in inputs.items():
            path = PurePosixPath(name)
            require(isinstance(name, str) and name.startswith(INPUT_ROOT)
                    and path.as_posix() == name and not path.is_absolute() and ".." not in path.parts
                    and matches(r"[a-f0-9]{64}", digest), "Retirement packaging input is invalid")
        seen.add(component)
        identifiers.add(entry["id"])
    require(seen == RETIRED_COMPONENTS, "Retirement policy omitted an authorized decision")
    replacements = {(entry["replacement_pr"], canonical(entry["packaging_inputs"])) for entry in policy["decisions"]}
    require(len(replacements) == 1, "Runtime replacements must share one reviewed packaging change")
    return policy


def blob(root, revision, name):
    require(matches(r"[0-9a-f]{40}", revision), "Retirement input revision is invalid")
    result = subprocess.run(["git", "show", revision + ":" + name], cwd=root,
                            capture_output=True, check=False, timeout=30)
    require(result.returncode == 0 and len(result.stdout) <= 2 * 1024 * 1024,
            "Retirement packaging input cannot be read")
    return result.stdout


def load_policy(root):
    path = root / MANIFEST
    require(path.is_file() and not path.is_symlink(), "Retirement policy is unavailable")
    policy = parse_policy(path.read_bytes())
    parent = subprocess.run(["git", "rev-parse", "HEAD^1"], cwd=root, capture_output=True,
                            text=True, timeout=30, check=False)
    require(parent.returncode == 0 and matches(r"[a-f0-9]{40}", parent.stdout.strip()),
            "Retirement policy parent is unavailable")
    revision = parent.stdout.strip()
    listed = subprocess.run(["git", "ls-tree", "--name-only", revision, "--", str(MANIFEST)],
                            cwd=root, capture_output=True, text=True, timeout=30, check=False)
    require(listed.returncode == 0, "Retirement policy history is unavailable")
    if listed.stdout.strip():
        require(listed.stdout.strip() == str(MANIFEST), "Retirement policy history differs")
        preserve_decisions(parse_policy(blob(root, revision, str(MANIFEST))), policy)
    return policy


def preserve_decisions(previous, current):
    """Reviewed decisions cannot disappear or be rebound by a later workflow."""
    old = {entry["id"]: entry for entry in previous["decisions"]}
    new = {entry["id"]: entry for entry in current["decisions"]}
    require(all(new.get(key) == value for key, value in old.items()), "Existing retirement decision changed")


def selected_pairs(queue):
    require(isinstance(queue, dict) and matches(r"[0-9a-f]{40}", queue.get("head"))
            and isinstance(queue.get("changes"), list), "Retirement queue is invalid")
    pairs, sources = {}, set()
    for change in queue["changes"]:
        require(isinstance(change, dict) and set(change) == {"source", "tree", "deployables"}
                and matches(r"[0-9a-f]{40}", change["source"]) and matches(r"[0-9a-f]{40}", change["tree"])
                and change["source"] not in sources, "Retirement queue source is invalid")
        names = change["deployables"]
        require(isinstance(names, list) and all(isinstance(name, str) for name in names)
                and names == sorted(set(names)) and set(names).issubset(DEPLOYABLES.values()),
                "Retirement queue selection is invalid")
        sources.add(change["source"])
        for component, name in DEPLOYABLES.items():
            if name in names:
                pairs[(component, change["source"])] = change["tree"]
    return pairs


class Retirements:
    def __init__(self, policy, dispositions):
        self.policy = policy
        self.dispositions = dispositions
        self.pairs = {(entry["reservation"]["component"], entry["reservation"]["source"])
                      for entry in policy["decisions"]}

    def guard(self, component, source):
        require((component, source) not in self.pairs, "Retired source cannot be published or finalized")

    def inventory(self, finals, pending, is_active):
        for entry in self.policy["decisions"]:
            expected = entry["reservation"]
            pair = (expected["component"], expected["source"])
            require(not any((tag["receipt"]["identity"]["component"], tag["target"]) == pair for tag in finals),
                    "Retired source unexpectedly has a final version")
            owned = [item for item in pending if (item["component"], item["source"]) == pair]
            require(owned == [expected], "Retired source burn record changed, disappeared or was reallocated")
            require(not any(is_active(item) for item in owned), "Retired source still has an active owner")


def resolve_retirements(policy, queue, api, root, *, selector, read_blob=blob):
    pairs = selected_pairs(queue)
    history = api.history()
    head_position = history.position(queue["head"])
    manifest_digest = hashlib.sha256(canonical(policy).encode()).hexdigest()
    dispositions, replacements = [], {}
    for entry in policy["decisions"]:
        reservation = entry["reservation"]
        component, source = reservation["component"], reservation["source"]
        latest_run = api.request(f"actions/runs/{reservation['run_id']}")
        require(isinstance(latest_run, dict) and latest_run.get("id") == reservation["run_id"]
                and type(latest_run.get("run_attempt")) is int
                and latest_run["run_attempt"] >= reservation["run_attempt"]
                and latest_run.get("status") == "completed", "Retired source workflow has not drained")
        reference = api.request("git/ref/tags/" + entry["reservation_tag"])
        require(isinstance(reference, dict) and reference.get("ref") == "refs/tags/" + entry["reservation_tag"]
                and isinstance(reference.get("object"), dict) and reference["object"].get("type") == "tag"
                and reference["object"].get("sha") == entry["reservation_object"], "Retired burn tag object changed")
        require(pairs.get((component, source)) == reservation["tree"], "Retired source disappeared from selected history")
        number = entry["replacement_pr"]
        if number not in replacements:
            pr = api.request(f"pulls/{number}")
            require(isinstance(pr, dict) and pr.get("number") == number and pr.get("merged") is True
                    and exact_base(pr) and matches(r"[a-f0-9]{40}", pr.get("merge_commit_sha")),
                    "Retirement replacement is not a merged develop PR")
            revision = pr["merge_commit_sha"]
            require(merged_pr(revision, api.request)["number"] == number,
                    "Retirement replacement merge is ambiguous")
            for name, digest in entry["packaging_inputs"].items():
                require(hashlib.sha256(read_blob(root, revision, name)).hexdigest() == digest,
                        "Retirement replacement packaging differs")
            replacements[number] = (revision, selector(root, revision))
        revision, selected = replacements[number]
        require(history.position(source) < history.position(revision) <= head_position,
                "Retirement replacement is outside planned develop history")
        require(DEPLOYABLES[component] in selected and (component, revision) in pairs,
                "Retirement replacement was not selected")
        commit = api.request("git/commits/" + revision)
        require(isinstance(commit, dict) and commit.get("sha") == revision
                and isinstance(commit.get("tree"), dict)
                and commit["tree"].get("sha") == pairs[(component, revision)],
                "Retirement replacement tree differs")
        dispositions.append({"id": entry["id"], "component": component, "source": source,
                             "tree": reservation["tree"], "status": "retired-unpublishable",
                             "manifest_sha256": manifest_digest, "replacement_pr": number,
                             "replacement_source": revision, "replacement_tree": pairs[(component, revision)]})
    return Retirements(policy, sorted(dispositions, key=lambda item: item["id"]))


def guard_retired(component, source, root=None):
    policy = load_policy(root or Path(__file__).resolve().parents[2])
    Retirements(policy, []).guard(component, source)
