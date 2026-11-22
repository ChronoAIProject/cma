"""Public, run-bound evidence for independent component admission."""
from pathlib import Path

from staging_cd_common import matches, require
from staging_cd_component_admission import validate_failures
from staging_cd_receipts import canonical
from staging_cd_set import validate_set
from staging_cd_targets import TARGETS


def write_admission(env, selected, value, failures, started):
    validate_failures(failures)
    receipts = validate_set(value)["receipts"] if value is not None else {}
    require(set(selected) <= TARGETS and len(selected) == len(set(selected))
            and not receipts.keys() & failures.keys()
            and receipts.keys() | failures.keys() == set(selected), "Admission report coverage differs")
    require(all(matches(r"[1-9][0-9]*", env.get(key)) for key in ("GITHUB_RUN_ID", "GITHUB_RUN_ATTEMPT"))
            and matches(r"[0-9a-f]{40}", env.get("GITHUB_SHA")), "Admission report run identity differs")
    require((value is None and started is None) or (value is not None and isinstance(started, str)),
            "Admission report clock differs")
    report = {"schema": "cma-component-admission-v1", "run_id": int(env["GITHUB_RUN_ID"]),
              "run_attempt": int(env["GITHUB_RUN_ATTEMPT"]), "event_source": env["GITHUB_SHA"],
              "selected": sorted(selected), "release_set": value, "failed": failures, "started_at": started}
    root = Path(env["RUNNER_TEMP"]) / "public-admission"
    require(not root.is_symlink(), "Admission report directory is invalid")
    root.mkdir(mode=0o700, exist_ok=True)
    path = root / "admission.json"
    require(not path.exists() and not path.is_symlink(), "Admission report must be fresh")
    path.write_text(canonical(report) + "\n", encoding="utf-8")
    return report
