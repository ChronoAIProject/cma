"""Transfer publication ownership of a complete AM/runtime transition."""
from staging_cd_common import matches, require
from staging_cd_compatibility import ContractPolicy, GROUP, RECIPE, VERSION_FILE
from staging_cd_contract import VERSIONS, compatibility
from staging_cd_reservations import next_reservation
from staging_cd_versions import DEPLOYABLES, label_level, merged_pr


def transition_in_window(root, history, changes):
    if not any(set(row["deployables"]) & {DEPLOYABLES[key] for key in GROUP} for row in changes):
        return None
    policy = ContractPolicy(history, root=root)
    sources = {row["source"] for row in changes}
    # Only merges touching a contract input can introduce a transition. Restrict
    # the search to the same first-parent adoption window as version allocation.
    first = min(sources, key=history.position)
    commits = policy.git("log", "--first-parent", "--max-count=2000", "--format=%H", history.head,
                         "--", VERSION_FILE, RECIPE).splitlines()
    for source in commits:
        if source not in history.positions or history.position(source) < history.position(first):
            break
        if source in sources and policy.transition(source):
            return source
    return None


def complete_group(anchor, jobs, changes, api, store, finals, pending, run_id, attempt):
    if anchor is None or not any(job["component"] in GROUP for job in jobs):
        return False
    history = api.history()
    start = history.position(anchor)
    require(any(change["source"] == anchor for change in changes), "Transition is outside the release queue")
    targets = {component: change for change in changes if history.position(change["source"]) >= start
               for component in GROUP if DEPLOYABLES[component] in change["deployables"]}
    require(targets.keys() == GROUP, "Transition is missing a selected publication source")
    by_component = {job["component"]: job for job in jobs}
    for component in sorted(GROUP):
        change = targets[component]
        source = change["source"]
        if component in by_component:
            job = by_component[component]
            require(job.get("receipt", job.get("reservation"))["source"] == source, "Transition source differs")
            continue
        published = [tag["receipt"] for tag in finals if tag["receipt"]["identity"]["component"] == component]
        known = [item for item in pending if item["component"] == component] + [r["identity"] for r in published]
        require(all(item["source"] not in history.positions or history.position(item["source"]) <= history.position(source)
                    for item in known), "Transition peer has a newer source")
        current = [receipt for receipt in published if receipt["source"] == source]
        require(len(current) <= 1, "Source already has conflicting final versions")
        if current:
            jobs.append({"component": component, "mode": "resume", "receipt": current[0]})
            continue
        # A same-source peer may still be queued or building in an older run.
        # Claim a fresh version so this group can finish if that peer fails.
        # A queued peer yields to this claim. A started one finishes first
        # under its lane lock; this claim then skips before building, and a
        # later allocation resumes the peer's final for the group.
        pr = merged_pr(source, api.request)
        provenance = next((tag["receipt"]["identity"] for tag in finals if tag["target"] == source), None)
        level = provenance["bump"] if provenance else label_level(pr)
        value = next_reservation(component, source, change["tree"], pr["number"], level,
                                 run_id, attempt, finals, pending)
        require(store.insert(value), "Allocation reservation collision")
        pending.append(value)
        jobs.append({"component": component, "mode": "publish", "reservation": value})
    return True


def verify_transition(anchor, receipts, policy):
    require(matches(r"[0-9a-f]{40}", anchor) and GROUP <= receipts.keys() and policy.transition(anchor),
            "Contract transition evidence is incomplete")
    signature = policy.signature(anchor)
    expected = tuple(dict(signature[0])[key] for key in VERSIONS)
    for key in GROUP:
        receipt = receipts[key]
        require(policy.history.position(receipt["source"]) >= policy.history.position(anchor)
                and policy.signature(receipt["source"]) == signature and compatibility(receipt) == expected,
                "Contract transition evidence differs")
