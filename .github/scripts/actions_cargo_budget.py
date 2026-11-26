"""Reserve the complete stored size of optional develop Cargo uploads."""

import json
import stat
from dataclasses import dataclass
from pathlib import Path

from actions_cache_retention import BUDGET, CARGO, DEVELOP
from staging_cd_common import matches, require
from staging_cd_github import GitHubNotFound

MIB = 1024**2
GIB = 1024**3


# Three short regular paths, directory entries and tar padding fit in 1 MiB.
# zstd's compressBound overhead is < 1% for large inputs; also cover small frames.
def stored_bound(payload):
    return payload + payload // 32 + MIB


MAX_CHARGE = stored_bound(GIB + 4096)
REF = "heads/automation/cargo-cache-budget-v4"
SCHEMA = "cargo-cache-budget-v4"
COMMIT_HEADER = "Account for outstanding develop Cargo uploads\n\n"


def bundle_bound(directory, key):
    files = {"manifest.json", "registry.tar.gz", "target.tar.gz"}
    require(
        directory.is_dir() and not directory.is_symlink() and {p.name for p in directory.iterdir()} == files,
        "Unexpected Cargo save directory",
    )
    sizes = {}
    for name in files:
        item = (directory / name).lstat()
        require(stat.S_ISREG(item.st_mode) and item.st_nlink == 1, "Invalid Cargo save member")
        sizes[name] = item.st_size
    require(
        0 < sizes["manifest.json"] < 4096 and sizes["registry.tar.gz"] + sizes["target.tar.gz"] <= GIB,
        "Cargo save exceeds archive limit",
    )
    require(
        json.loads((directory / "manifest.json").read_bytes())["prefix"] == key.rsplit("-", 2)[0] + "-",
        "Cargo save manifest differs from grant",
    )
    return stored_bound(sum(sizes.values()))


def owner(key):
    m = CARGO.fullmatch(key)
    require(m is not None, "Invalid Cargo archive identity")
    _, run, attempt = key.rsplit("-", 2)
    require(matches(r"[1-9][0-9]*", run) and matches(r"[1-9][0-9]*", attempt), "Invalid Cargo writer identity")
    return int(run), int(attempt), m[1]


@dataclass
class Account:
    charges: dict

    @classmethod
    def decode(cls, doc):
        require(
            isinstance(doc, dict) and set(doc) == {"schema", "charges"} and doc["schema"] == SCHEMA,
            "Unknown cache budget format",
        )
        charges = doc["charges"]
        require(isinstance(charges, dict) and len(charges) <= 1024, "Invalid cache budget charges")
        for key, limit in charges.items():
            owner(key)
            require(type(limit) is int and 0 < limit <= MAX_CHARGE, "Invalid cache upload limit")
        return cls(charges)

    def encode(self):
        return {"schema": SCHEMA, "charges": self.charges}


class BudgetRef:
    """Only the independent admission job can advance this fast-forward-only ref."""

    def __init__(self, api, *, ref=REF, account_type=Account, header=COMMIT_HEADER):
        self.api = api
        self.ref = ref
        self.account_type = account_type
        self.header = header

    def read(self):
        try:
            ref = self.api.request("git/ref/" + self.ref, allow_not_found=True)
        except GitHubNotFound:
            return None, self.account_type({})
        tip = ref["object"]["sha"]
        require(
            ref["ref"] == "refs/" + self.ref and ref["object"]["type"] == "commit" and matches("[a-f0-9]{40}", tip),
            "Invalid cache budget ref",
        )
        commit = self.api.request("git/commits/" + tip)
        require(commit["sha"] == tip, "Cache budget commit differs")
        require(commit["message"].startswith(self.header), "Cache budget revision has no accounting record")
        return tip, self.account_type.decode(json.loads(commit["message"][len(self.header) :]))

    def advance(self, previous, account, source):
        # Sibling commits cannot fast-forward each other. An ambiguous response
        # can leave a charge in the ledger, but cannot create an uncharged grant.
        state = self.account_type.decode(account.encode()).encode()
        tree = self.api.request("git/commits/" + (previous or source))["tree"]["sha"]
        require(matches("[a-f0-9]{40}", tree), "Invalid budget tree")
        commit = self.api.request(
            "git/commits",
            {
                "message": self.header + json.dumps(state, sort_keys=True),
                "tree": tree,
                "parents": [previous or source],
            },
        )
        oid = commit["sha"]
        require(matches("[a-f0-9]{40}", oid), "Invalid budget commit")
        if previous:
            result = self.api.request("git/refs/" + self.ref, {"sha": oid, "force": False}, method="PATCH")
        else:
            result = self.api.request("git/refs", {"ref": "refs/" + self.ref, "sha": oid})
        require(result["ref"] == "refs/" + self.ref and result["object"]["sha"] == oid, "Budget grant was not committed")


def grant(account, entries, candidates, skipped):
    used = sum(row.size for row in entries)
    visible = {r.key: r.size for r in entries if r.ref == DEVELOP and r.size and CARGO.fullmatch(r.key)}
    for key, cap in list(account.charges.items()):
        if key in visible:
            require(visible[key] <= cap, "Stored Cargo upload exceeded its grant")
            del account.charges[key]
        elif skipped(owner(key)):
            del account.charges[key]
    outstanding = sum(account.charges.values())
    accounted = used + outstanding
    admitted = {}
    # The threshold applies before a save. Reserve its entire possible upload
    # before considering another candidate, even if its row is still absent.
    candidates = [
        k
        for k in candidates
        if k not in account.charges and not any(v.startswith(k.rsplit("-", 2)[0] + "-") for v in visible)
    ]
    for key in candidates:
        owner(key)
        if accounted >= BUDGET:
            break
        account.charges[key] = admitted[key] = MAX_CHARGE
        accounted += MAX_CHARGE
    return {
        "inventory_bytes": used,
        "outstanding_bytes": outstanding,
        "admitted_bytes": sum(admitted.values()),
        "grants": len(admitted),
        "budget_bytes": BUDGET,
    }


def save_decision(api, ledger, env, directory, key):
    run, attempt, service = owner(key)
    require(
        (str(run), str(attempt), service) == (env["GITHUB_RUN_ID"], env["GITHUB_RUN_ATTEMPT"], env["CD_COMPONENT"]),
        "Cargo grant owner differs",
    )
    revision, account = ledger.read()
    cap = account.charges.get(key, 0)
    result = {"allowed": False, "grant_bytes": cap, "component": service}
    if not cap:
        return dict(result, reason="no-current-grant")
    result["bundle_bound_bytes"] = bundle_bound(Path(directory), key)
    if result["bundle_bound_bytes"] > cap:
        return dict(result, reason="bundle-exceeds-grant")
    # A concurrent grant can be absent from both the first ledger read and
    # the cache census. Only combine observations from one ledger revision.
    entries = api.entries()
    result["inventory_bytes"] = sum(e.size for e in entries)
    current_revision, _ = ledger.read()
    if current_revision != revision:
        return dict(result, reason="budget-revision-changed")
    visible = {e.key for e in entries if e.ref == DEVELOP and e.size}
    result["outstanding_bytes"] = sum(limit for k, limit in account.charges.items() if k != key and k not in visible)
    if key in visible:
        return dict(result, reason="already-visible")
    if result["inventory_bytes"] + result["outstanding_bytes"] >= BUDGET:
        return dict(result, reason="repository-headroom")
    return dict(result, allowed=True, reason="reserved")
