"""Publish final digest-bound version tags, then remove successful reservations."""
import json
import os
import time
from pathlib import Path

from staging_cd_admission import event_identity
from staging_cd_common import require
from staging_cd_github import GitHub
from staging_cd_release_tags import local_tag_reader
from staging_cd_finalize_diagnostics import run_finalization
from staging_cd_receipts import canonical, final_tag_oid, validate_receipt
from staging_cd_registry import Registry
from staging_cd_reservations import Reservations
from staging_cd_resolve import output

# The job allows fifteen minutes, including checkout and the receipt download.
# GitHub and registry reads share this budget and refuse cleanly once it is spent.
FINALIZE_READ_SECONDS = 600


def finalize(tasks, receipts, publisher, registry, *, recovery_oids=None, mark=lambda _stage: None):
    mark("receipt")
    require(isinstance(tasks, list) and isinstance(receipts, list) and 0 < len(receipts) <= len(tasks),
            "Published receipt set is invalid")
    recovery_oids = {} if recovery_oids is None else recovery_oids
    seen = set()
    for task, receipt in zip(tasks, receipts):
        mark("receipt")
        validate_receipt(receipt)
        mark("allocation")
        require(receipt["tag"] not in seen and receipt["identity"]["component"] == task["component"],
                "Publication receipts are reordered or duplicated")
        seen.add(receipt["tag"])
        if task["mode"] == "resume":
            require(receipt == task["receipt"], "Resumed receipt changed")
        else:
            expected = dict(task["reservation"], schema="cma-image-release-v1", build=receipt["identity"]["build"])
            require(receipt["identity"] == expected, "Published receipt differs from the allocation owner")
        publisher.finalize(receipt, registry.exists, recovery_oid=recovery_oids.get(receipt["tag"]))
    return receipts[-1]


def main():
    env = os.environ
    def operation(_masker, diagnostics):
        event_identity(env)
        require(env["GITHUB_EVENT_NAME"] == "push", "Final version tags only accept develop pushes")
        deadline = time.monotonic() + FINALIZE_READ_SECONDS
        api = GitHub(env.get("GH_TOKEN"), local_tags=local_tag_reader(env.get("CD_COMPONENT"), Path.cwd()),
                     deadline=deadline)
        publisher = Reservations(api, env.get("GH_TOKEN"), Path.cwd(), mark=diagnostics.mark)
        diagnostics.mark("artifact")
        path = Path(env["CD_RECEIPTS_PATH"])
        require(path.is_file() and not path.is_symlink() and path.stat().st_size <= 1024 * 1024,
                "Published receipt artifact is unavailable")
        receipts = json.loads(path.read_text())
        diagnostics.mark("attempt")
        recovery_oids = {}
        for receipt in receipts:
            validate_receipt(receipt)
            if (receipt["identity"]["run_id"], receipt["identity"]["run_attempt"]) != \
                    (int(env["GITHUB_RUN_ID"]), int(env["GITHUB_RUN_ATTEMPT"])):
                # Failed attempts burn unfinalized reservations. A retry may
                # reuse a final receipt, but cannot revive an orphaned push.
                existing = [tag for tag in api.tags(receipt["identity"]["component"])
                            if tag["name"] == receipt["tag"]]
                expected_oid = final_tag_oid(receipt)
                require(len(existing) == 1 and existing[0]["receipt"] == receipt,
                        "A failed publication attempt burned this unfinalized version")
                require(existing[0].get("oid") == expected_oid,
                        "Prior-attempt final tag object differs")
                recovery_oids[receipt["tag"]] = expected_oid
        receipt = finalize(json.loads(env["CD_TASKS"]), receipts, publisher, Registry(deadline=deadline),
                           recovery_oids=recovery_oids, mark=diagnostics.mark)
        diagnostics.mark("output")
        output(env, "receipt", canonical(receipt))
        output(env, "component", receipt["identity"]["component"])
        return True
    return run_finalization(operation)


if __name__ == "__main__":
    raise SystemExit(main())
