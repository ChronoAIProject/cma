"""Every selected source is published or explicitly retired with a real successor."""
import json
import os
from pathlib import Path

from staging_cd_admission import event_identity
from staging_cd_allocate import run_active
from staging_cd_common import require
from staging_cd_github import GitHub
from staging_cd_queue import selection
from staging_cd_receipts import validate_receipt
from staging_cd_registry import Registry
from staging_cd_reservations import Reservations
from staging_cd_retirements import load_policy, resolve_retirements, selected_pairs
from staging_cd_runtime import run_private


def account(queue, retirements, finals, pending, is_active, exists):
    require(queue.get("dispositions") == retirements.dispositions, "Release dispositions differ")
    retirements.inventory(finals, pending, is_active)
    selected = selected_pairs(queue)
    required = set(selected) - retirements.pairs
    require(retirements.pairs.issubset(selected), "Retirement partition is incomplete")
    published = {}
    for tag in finals:
        receipt = validate_receipt(tag["receipt"])
        pair = (receipt["identity"]["component"], receipt["source"])
        if pair not in required:
            continue
        require(pair not in published and tag["target"] == receipt["source"]
                and tag["name"] == receipt["tag"] and receipt["identity"]["tree"] == selected[pair],
                "Release accounting found a conflicting final")
        require(exists(receipt["repository"], receipt["digest"]) is True,
                "Required image is not anonymously available")
        published[pair] = receipt
    require(set(published) == required, "Required publication is incomplete")
    for disposition in retirements.dispositions:
        replacement = (disposition["component"], disposition["replacement_source"])
        require(replacement in published
                and published[replacement]["identity"]["pr"] == disposition["replacement_pr"]
                and published[replacement]["identity"]["tree"] == disposition["replacement_tree"],
                "Retired source lacks its genuine replacement publication")
    return {"selected": len(selected), "published": len(published), "retired": len(retirements.pairs)}


def main():
    env, root = os.environ, Path.cwd()
    def operation(_masker):
        event_identity(env)
        require(env["GITHUB_EVENT_NAME"] == "push", "Release accounting only accepts develop pushes")
        queue = json.loads(env["CD_QUEUE"])
        api = GitHub(env.get("GH_TOKEN"))
        retirements = resolve_retirements(load_policy(root), queue, api, root, selector=selection)
        pending = Reservations(api, env.get("GH_TOKEN"), root).list()
        account(queue, retirements, api.tags(), pending, lambda item: run_active(api, item), Registry().exists)
        return True
    return run_private(operation, None)


if __name__ == "__main__":
    raise SystemExit(main())
