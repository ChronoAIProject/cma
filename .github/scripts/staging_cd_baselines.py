"""Keep each lane's newest durable version record selected for that lane."""
from staging_cd_common import require
from staging_cd_versions import DEPLOYABLES


def durable_baselines(changes, pending, finals, history):
    """Return the ordered queue with each lane's newest record source selected.

    Every reservation was made at a source its lane's selector then chose. A
    narrower selector (publisher-only changes stopped selecting every image)
    can leave a lane's newest final, or its newest failed attempt, at a source
    that no longer selects it. Selecting that source for that lane alone
    reproduces the allocation the record was made under: a current lane stays
    current or resumes its own final, a failed attempt retries at its own
    source, and a contract-group target never falls behind a peer's record.
    Under an unchanged selector every record is already selected.
    """
    if not changes:
        return changes
    first = history.position(changes[0]["source"])
    selected = {name: history.position(change["source"]) for change in changes for name in change["deployables"]}
    newest = {}
    for item in [*pending, *(tag["receipt"]["identity"] for tag in finals)]:
        component, source = item["component"], item["source"]
        # Legacy records outside history order nothing; records older than the
        # reconciled window keep their existing (unselected) treatment.
        if component not in DEPLOYABLES or source not in history.positions:
            continue
        position = history.position(source)
        floor = max(selected.get(DEPLOYABLES[component], first - 1), newest.get(component, (first - 1,))[0])
        if position >= first and position > floor:
            newest[component] = (position, item)
    if not newest:
        return changes
    rows = {change["source"]: change for change in changes}
    for component, (_position, item) in newest.items():
        row = rows.get(item["source"], {"source": item["source"], "tree": item["tree"], "deployables": []})
        require(row["tree"] == item["tree"], "Release change identity is invalid")
        rows[item["source"]] = dict(row, deployables=sorted({*row["deployables"], DEPLOYABLES[component]}))
    return sorted(rows.values(), key=lambda change: history.position(change["source"]))
