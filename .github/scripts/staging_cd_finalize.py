"""Publish final digest-bound version tags, then remove successful reservations."""
import json
import os
from pathlib import Path

from staging_cd_admission import event_identity
from staging_cd_common import require
from staging_cd_github import GitHub
from staging_cd_receipts import canonical, validate_receipt
from staging_cd_registry import Registry
from staging_cd_reservations import Reservations
from staging_cd_resolve import output
from staging_cd_runtime import run_private


def finalize(tasks, receipts, publisher, registry):
    require(isinstance(tasks, list) and isinstance(receipts, list) and 0 < len(receipts) <= len(tasks),
            "Published receipt set is invalid")
    seen = set()
    for task, receipt in zip(tasks, receipts):
        validate_receipt(receipt)
        require(receipt["tag"] not in seen and receipt["identity"]["component"] == task["component"],
                "Publication receipts are reordered or duplicated")
        seen.add(receipt["tag"])
        if task["mode"] == "resume":
            require(receipt == task["receipt"], "Resumed receipt changed")
        else:
            expected = dict(task["reservation"], schema="cma-image-release-v1", build=receipt["identity"]["build"])
            require(receipt["identity"] == expected, "Published receipt differs from the allocation owner")
        publisher.finalize(receipt, registry.exists)
    return receipts[-1]


def main():
    env = os.environ
    def operation(_masker):
        event_identity(env)
        require(env["GITHUB_EVENT_NAME"] == "push", "Final version tags only accept develop pushes")
        api = GitHub(env.get("GH_TOKEN"))
        publisher = Reservations(api, env.get("GH_TOKEN"), Path.cwd())
        path = Path(env["CD_RECEIPTS_PATH"])
        require(path.is_file() and not path.is_symlink() and path.stat().st_size <= 1024 * 1024,
                "Published receipt artifact is unavailable")
        receipts = json.loads(path.read_text())
        for receipt in receipts:
            validate_receipt(receipt)
            if (receipt["identity"]["run_id"], receipt["identity"]["run_attempt"]) != \
                    (int(env["GITHUB_RUN_ID"]), int(env["GITHUB_RUN_ATTEMPT"])):
                # Failed attempts burn unfinalized reservations. A retry may
                # reuse a final receipt, but cannot revive an orphaned push.
                existing = [tag for tag in api.tags(receipt["identity"]["component"])
                            if tag["name"] == receipt["tag"]]
                require(len(existing) == 1 and existing[0]["receipt"] == receipt,
                        "A failed publication attempt burned this unfinalized version")
        receipt = finalize(json.loads(env["CD_TASKS"]), receipts, publisher, Registry())
        output(env, "receipt", canonical(receipt))
        output(env, "component", receipt["identity"]["component"])
        return True
    return run_private(operation, None)


if __name__ == "__main__":
    raise SystemExit(main())
